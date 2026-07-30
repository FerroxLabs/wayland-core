# NOTES — lane/glibc-reach

Live investigation log. Appended and re-committed after every measurement (LANE-BRIEF §6b-i).

Base: `5cd37f79`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-glibc-reach`.

---

## Goal

Lower the glibc floor of the shipped Linux release binaries for BOTH
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, and prove the new floor by
measurement (`readelf -V`), in both directions (a floor check that cannot redden is worthless).

## Brief premises — to verify before acting (LANE-BRIEF: "your brief's measurements are stale")

| # | Claim in brief | Status |
|---|---|---|
| P1 | `release.yml` builds both Linux targets on `ubuntu-latest` | **VERIFIED** by reading `.github/workflows/release.yml:63-69`. Both `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` are `os: ubuntu-latest`. |
| P2 | Shipped binary max symbol = `GLIBC_2.39` | UNVERIFIED — must measure myself at my own base SHA. |
| P3 | *Both* Linux targets therefore have a 2.39 floor | **SUSPECT.** The aarch64 leg sets `use_cross: true` (`release.yml:69`) and runs `cross build`. A `cross` build links against the **container's** aarch64 sysroot, NOT the runner's glibc. `Cross.toml` sets `pre-build` and `env.passthrough` but **no `image =` override**, so the floor is whatever the default `cross` image ships. The runner's 2.39 is irrelevant to that leg. Must be measured separately. |

P3 matters a lot: if aarch64 is already low, the fix is x86_64-only and much cheaper, and the
brief's framing ("lower it for both") would be half wrong.

## Environment facts established

- hetzner-dsm: Ubuntu 24.04 noble, 96 cores, 996G free on `/`, **Docker 29.2.1 present** (no podman).
  Ubuntu 24.04 == what `ubuntu-latest` currently resolves to, so a native hetzner build reproduces
  the x86_64 release leg's environment exactly.
- hetzner `/root/wayland` remote refspec is `+refs/heads/*:refs/remotes/origin/*` — WIDE, so the
  §2a narrow-refspec trap does not apply here. Assert the SHA after checkout anyway.

## Instrument discipline for this lane

My entire deliverable is `readelf | grep | sort | tail`, which is the exact pipeline shape `rtk`
is known to corrupt (LANE-BRIEF §3b: `--numstat` fabricated `162 0`; `grep -c` returned 0 for a
present string). Therefore:

- Every measurement is **redirected to a file on the build host**, copied back, and read with the
  **Read tool** — never rendered through a Bash pipeline.
- Every capture carries a **known-positive and a known-negative** in the same file.
- `${PIPESTATUS[0]}` is not used (dies in dash); anything needing it runs under `bash -c`.

## Status

- [x] Read LANE-BRIEF, MILESTONE-SHIP, release.yml, Cross.toml
- [x] P1 verified from source
- [ ] P2/P3 measured
- [ ] Option chosen and justified
- [ ] New floor measured for both targets
- [ ] CI gate added, proven in both directions
