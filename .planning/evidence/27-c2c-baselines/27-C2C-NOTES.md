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

## Open questions being worked

- Can a standalone Chrome-for-Testing / `chrome-headless-shell` zip be fetched to
  hetzner (no root install, no snap) to give the browser leg a REAL backend?
- Does the reaper leg need a browser at all, or can the supervisor be driven against an
  arbitrary real child process? (`reaper_terminates_orphans_with_dead_parent`,
  `supervisor.rs:646`, suggests it can.)

## Standard I am holding myself to

Every gate run in BOTH directions (LANE-BRIEF §3b-iii): construct the failing world and
watch it redden, AND construct the passing world and watch it green. A skip is reported
as not-run, never as a pass. All reported numbers come from unproxied tools.
