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

## Standard I am holding myself to

Every gate run in BOTH directions (LANE-BRIEF §3b-iii): construct the failing world and
watch it redden, AND construct the passing world and watch it green. A skip is reported
as not-run, never as a pass. All reported numbers come from unproxied tools.
