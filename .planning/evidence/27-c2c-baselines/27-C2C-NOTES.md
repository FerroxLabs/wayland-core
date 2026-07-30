# 27-C2(c) — three policy baselines — WORKING NOTES

Lane `27c2c-baselines`, branch `lane/27c2c-baselines`, base `8955ee6e`.
Append-only. Re-committed after every measurement (LANE-BRIEF §6b-i).

## The criterion clause I own

`27-C2`: *"Browser, CUA, and web surfaces publish live readiness and preserve sandbox,
egress, approval, and cleanup policy."* (a) and (b) CLOSED. (c) is three baselines with
**no measurement at all**:

1. downloads-root confinement on a browser download;
2. the approval gate on a computer-use operation;
3. process count before / during / after a session, plus one reaper interval.

## Premise check on the brief (LANE-BRIEF "your brief's MEASUREMENTS are probably stale")

The ledger's parked blocker is *"two of three legs are blocked on a display-capable
host that hetzner cannot provide."* **Verified on `hetzner-dsm` at session start —
partly FALSE, partly TRUE, and the two halves point in opposite directions:**

| Probe | Result |
|---|---|
| `/usr/bin/Xvfb` | PRESENT |
| `/usr/bin/xvfb-run` | PRESENT |
| `/usr/lib/x86_64-linux-gnu/libXtst.so.6` | PRESENT (XTest = what the X11 CUA backend uses) |
| `camoufox` on PATH | MISSING |
| `/root/.cache/camoufox` | absent |
| `chromium`,`chromium-browser`,`google-chrome{,-stable}`,`chrome`,`firefox` on PATH | ALL MISSING |
| `apt-cache policy chromium` | `Candidate: (none)` |
| `apt-cache policy chromium-browser` | `2:1snap1-0ubuntu2` — snap transitional only |
| `/root/.cache/ms-playwright` | absent |

So: **the display half of the blocker is false** (hetzner can host a display), and the
**browser-binary half is true but was never the stated reason.** The CUA leg is
unblocked. The browser leg is blocked on a *binary*, not a *display*.

`crates/wcore-cua/src/backends/linux_x11.rs:159-166` — the only display gate is `DISPLAY`
being unset, returning a typed `UnsupportedPlatform`. `xvfb-run` sets `DISPLAY`.
`crates/wcore-cua/Cargo.toml` — `x11` is in `default`, plus an opt-in `x11-test` feature.

## Where the three enforcement points actually live

1. **Downloads-root** — `crates/wcore-browser/src/tool.rs:112 validate_local_path(raw,
   downloads_root)`; root-confinement block at `:151-162`, symlink-aware via
   `canonicalize_existing_prefix` (`:188`). Called from the tool dispatch at `:471`.
   Fails closed: `BrowserTool::new` sets `downloads_root:
   Some(default_downloads_root())` (`:258`, `:178`). Existing unit tests at `:790`,
   `:805`, `:864` — but they are unit tests of the helper, not a recorded baseline.
2. **CUA approval** — `crates/wcore-cua/src/policy.rs`: `require_approval_for_app`
   (`:371-380`), `first_time_per_app_approval` (`:383-395`), unresolved-frontmost
   fail-closed (`:359-368`). All route to `Suspend`.
3. **Reaper / process count** — `crates/wcore-browser/src/supervisor.rs`:
   `start_reaper` (`:185`), `SupervisorConfig::reaper_interval` default 1s (`:37,:53`).

## UPDATE 1 — the browser blocker is fully falsified: the REAL Camoufox sidecar runs on hetzner

`@askjo/camofox-browser` — the exact package `supervisor.rs:309` tells operators to install —
**exists on npm at 1.13.0 and runs on `hetzner-dsm`.**

```
npm install --prefix /root/27c2c-tools @askjo/camofox-browser@1.13.0   # added 194 packages in 22s, rc=0
/root/27c2c-tools/node_modules/.bin/camofox-browser --port 9377
curl -s -w "HTTP=%{http_code}" http://127.0.0.1:9377/health
  HTTP=200
  {"ok":true,"engine":"camoufox","browserConnected":true,"browserRunning":true,
   "activeTabs":0,"activeSessions":0,"consecutiveFailures":0,...}
```

Its own log: `"xvfb virtual display started","display":":99"` then
`"camoufox launched" ... "browser pre-warmed","ms":1153`. Process table shows a real tree:

```
3811470 3778830 Xvfb
3811471 3778830 camoufox-bin
```

So **neither leg of the parked blocker survives.** hetzner can host a display (Xvfb+XTest,
verified above) *and* the real primary browser backend. This is the production topology, not
a stand-in — `SupervisorConfig::local_camoufox` (`supervisor.rs:66`) spawns exactly this
program and health-gates it on exactly this `/health`.

## UPDATE 2 — a finding that reshapes baseline 1: NO backend implements `BrowserOp::Download`

Measured, with a known-positive control on the instrument in the same invocation.

- **Chromium** — explicit: `backends/chromium.rs:332-338` returns
  `Unsupported("chromium: Download requires Browser.downloadProgress event handling; not in
  chromiumoxide v0.7 public surface. Use Camoufox.")`. `Upload` likewise at `:327`.
- **Camoufox** — falls through the catch-all. Enumerating the arms of
  `backends/camoufox.rs:240-425 dispatch()` gives exactly: `Navigate`, `Snapshot`,
  `Read`(→Unsupported), `GetState`, `Click`, `Fill`, `Press`, `Screenshot`, `Back`/`Forward`,
  then `unsupported => Err(Unsupported("{op} does not have a truthful Camoufox API mapping"))`.
  **`Download` and `Upload` are not among them.** `op_name()` maps `Download => "download"`
  (`:503`) *only to name it in that error string*.
  Instrument control: the same `sed|grep` finds `BrowserOp::Navigate` (count 1), so it is alive.

**Consequence for the criterion.** *"A browser download must land inside the configured
downloads root and must not escape it"* splits into two halves with different fates:

- **"must not escape"** — fully measurable. The gate is `validate_local_path`, invoked at
  `tool.rs:471` **before any backend dispatch**, so it is reachable and decisive today.
- **"must land inside"** — **NOT measurable end-to-end in the product as shipped**, because no
  shipped backend can perform a download at all. I will measure the tool-layer half (the
  normalized in-root path is what reaches the provider, and a write to it lands in-root) and
  I will **not** claim the end-to-end half. This is reported, not papered over.

## Plan (all three, both directions)

1. **Downloads-root** — `crates/wcore-browser/tests/downloads_root_baseline_test.rs`, recording
   provider that really writes bytes. Escape arms (absolute, `..`, dotfile, **symlink**) must
   redden AND reach the provider 0 times; in-root arm must green AND reach the provider once
   with a normalized path. **The discrimination control:** the *same* target path is REFUSED
   under root A and ADMITTED under root B (= its own parent) — proving the verdict is caused by
   the root boundary, not by an unrelated shape check.
2. **CUA approval** — `crates/wcore-cua/tests/approval_gate_baseline_test.rs`, feature
   `x11-test`, under `xvfb-run`. Observable is the **real X11 pointer position** read back with
   `query_pointer`, not the Rust return value. Withheld ⇒ `PolicySuspended` AND pointer
   unmoved; granted ⇒ `Ok` AND pointer at the requested coordinate. The granted arm is the
   known-positive that makes "unmoved" mean something.
3. **Process count + reaper** — driven against the **real Camoufox sidecar** through the real
   `BrowserSupervisor`. before / during / after by real PIDs in `/proc`, plus an orphan arm
   (dead parent ⇒ reaped within one interval) and a live-parent control arm (⇒ NOT reaped).

## UPDATE 3 — measurement-hygiene corrections applied mid-lane

**(a) rtk fabricates machine-readable counts even through absolute paths and pipes**
(orchestrator correction, `LANE-BRIEF.md` @ `27c30527`). Every number in this lane is now
captured by **redirecting to a file on the build host, `scp`-ing the file into
`.planning/evidence/27-c2c-baselines/`, and reading it with the Read tool** — never through
a Bash-rendered `grep`/`cat`. The raw captures are committed alongside this file as
`raw-*.log` / `raw-*.txt` so any reader can re-check the figures against the source.
Re-verified this way, retroactively: `raw-ev1-downloads-root.log` and
`raw-ev2a-approval-tool-level.log` — both agree with what was reported (`2 passed; 0 failed;
0 ignored; 0 measured; 0 filtered out` and `1 passed; 0 failed; 0 ignored; 0 measured;
0 filtered out` respectively). No fabrication found in these two, but the numbers are now
sourced correctly regardless.

**(b) Which config layer the approval baseline reads.** Relevant to the
`lane/egress-merge-polarity` finding that an untrusted project config can RAISE
`max_tokens`/`max_turns`. **My baseline reads no merged config at all** — it constructs
`CuaPolicy` programmatically in-process (`CuaPolicy::permissive()` then setting
`require_approval_for_app` / `first_time_per_app_approval` directly). So the measurement is
NOT sitting on the mis-polarisation defect. **Stated as a limitation, not a strength:** the
baseline therefore does NOT prove that the config→`CuaPolicy` plumbing preserves approval
settings across the trust boundary. If an untrusted project config can weaken
`require_approval_for_app` or flip `first_time_per_app_approval` to `false`, this baseline
would not see it. That is an untested adjacent surface and I am recording it as such.

**(c) Base.** Integration moved `8955ee6e` → `690eb928` while I worked. This branch remains
based at `8955ee6e` per the orchestrator's instruction; no rebase (forbidden by §0 anyway).

## UPDATE 4 — baseline 1 CLOSED, both directions, with a known-negative mutation

Raw: `raw-ev1-downloads-root.log`, `raw-ev1-mutated-known-negative.log`.

**Can it pass?** ARM A (in-root dest) → admitted, provider reached **1** time with a
canonicalized in-root path, file landed with the expected bytes. ARM D (discrimination) →
the *same literal path* refused under root A is **admitted** under root B (its own parent),
provider reached 1, landed. So the verdict tracks the root boundary, not an unrelated check.

**Can it fail?** Four escape shapes — absolute, `..`-traversal, dotfile, and **symlink** —
all refused, **provider reached 0 times each**, **0 files landed outside the root**.

**Known-negative on my own instrument.** I replaced the symlink-resolving confinement in
`tool.rs` with a naive lexical `starts_with` on the unresolved path (the exact defect ARM C
describes) and re-ran: `MUTATED_RC=101`, `1 passed; 1 failed`, failing precisely at
`B4-symlink: escape to /tmp/.tmp0XjPCg/innocent/loot.bin was NOT refused`. Mutation reverted
(`tool.rs` clean; only a pre-existing `Cargo.lock` drift remains — see UPDATE 6), suite back
to `2 passed; 0 failed`. **So the gate is neither permanently green nor permanently red.**

## UPDATE 5 — baseline 2: tool level CLOSED; the X11 physical arm hit a real environment wall

Raw: `raw-ev2a-approval-tool-level.log`, `raw-ev2b-x11-FAILED-liveness.log`,
`raw-xtest-probe-xdotool.txt`, `raw-xtest-probe2.txt`.

**2a (tool level) — CLOSED, `1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.**
4 arms. Withheld (`require_approval_for_app`) ⇒ `PolicySuspended` with **backend dispatches
= 0**; granted ⇒ `Ok` with **backend dispatches = 1**; first-time-per-app before approval ⇒
`PolicySuspended`, dispatches 0; after `mark_app_seen` ⇒ `Ok`, dispatches 1. The
zero-dispatch figure is what makes this an enforcement measurement rather than an
error-string check: approval is not advisory, the op never reaches the desktop.

**2b (real X11) — my instrument-liveness step FAILED, and it was right to.** Under
`xvfb-run -a -s "-screen 0 1280x1024x24"` with `--features x11-test`, `2 tests` compiled and
ran (so the feature gate works and nothing was silently skipped). The product's `MouseMove
{x:100,y:100}` returned **`Ok`**, but `QueryPointer` read **`(640, 512)`** — the untouched
screen centre. **A green return with no desktop effect.** Had I asserted only on the Rust
return value, this arm would have "passed" while measuring nothing.

Cause is **environmental, not ours** — independently confirmed with a second instrument
(`raw-xtest-probe2.txt`): on this Xvfb `xdotool mousemove --sync 300 200`,
`xdotool mousemove_relative --sync 50 50` and `xdotool click 1` **all return rc=0 and all
leave the pointer pinned at `640,512`**. `xdpyinfo` confirms the server does advertise
`XTEST` among its 23 extensions, and the screen is the 1280x1024 I asked for. So pointer
*position* is not a usable observable on this display, for any tool.

Switching the observable from pointer *position* to **event delivery** (what a real client
actually sees) — see UPDATE 7.

## Standard I am holding myself to

Every gate run in BOTH directions (LANE-BRIEF §3b-iii): construct the failing world and
watch it redden, AND construct the passing world and watch it green. A skip is reported
as not-run, never as a pass. All reported numbers come from unproxied tools.
