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

## MEASUREMENT 1 — brief premises resolved

**P2 CONFIRMED.** `/root/wayland/target/release/wayland-core` (hetzner, mtime 2026-07-30 02:58Z)
references `GLIBC_2.39` as its maximum. Instrument controls in the same capture: known-positive
`GLIBC_2.2.5` → **62** hits; known-negative `GLIBC_9.99` → **0**. So the grep was alive and the
absence is real.

**P3 CONFIRMED — my suspicion was WRONG, and I am recording that.** I predicted the aarch64 leg
would already be low because it is a `cross` build. Measured instead: the default
`ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main` image is **Ubuntu 24.04 with a glibc 2.39
aarch64 sysroot** (`/usr/aarch64-linux-gnu/lib/libc.so.6` → "Ubuntu GLIBC 2.39-0ubuntu8"). Both
legs really are at 2.39. The brief was right and my inference was wrong.

**`cross` 0.2.5's image is Ubuntu 16.04 / glibc 2.23** — a very low floor, but its apt repos are
long dead (16.04 is EOL and archived) and `Cross.toml`'s `pre-build` does `apt-get update` plus
four arm64 `-dev` installs, so that image cannot satisfy this tree. It also predates OpenSSL 3
(see below), which rules it out independently.

## MEASUREMENT 2 — the finding that changes the answer: OpenSSL 3 is a HARD ABI FLOOR

The brief frames this as a pure glibc problem. It is not. `ldd` on the shipped binary:

```
libssl.so.3    => /lib/x86_64-linux-gnu/libssl.so.3
libcrypto.so.3 => /lib/x86_64-linux-gnu/libcrypto.so.3
libseccomp.so.2, libdbus-1.so.3, libgcc_s.so.1, libm.so.6, libc.so.6
```

`openssl-sys` + `native-tls` are in `Cargo.lock`, and the binary carries **`NEEDED libssl.so.3`**.
That soname only exists on OpenSSL 3 distros. So the build container's OpenSSL major version is a
second, independent reach constraint that moves in the OPPOSITE direction to glibc:

| Base | glibc | OpenSSL soname | Verdict |
|---|---|---|---|
| `rockylinux:8` / `almalinux:8` / manylinux_2_28 | 2.28 | `libssl.so.1.1` | **REJECTED** — the brief's "best reach" option would emit a binary needing `libssl.so.1.1`, which does **not exist** on Ubuntu 22.04, Debian 12 or RHEL 9. It trades a glibc break for an OpenSSL break **on exactly the distros we are trying to reach**, and is therefore *worse*, not better. |
| `debian:11` | 2.31 | `libssl.so.1.1` | REJECTED, same reason. |
| **`almalinux:9` / `rockylinux:9`** | **2.34** | `libssl.so.3` | **Best x86_64 option.** Lowest glibc that still carries OpenSSL 3. Supported to 2032. |
| `ubuntu:22.04` | 2.35 | `libssl.so.3` | Best easily-available **arm64 multiarch** base with OpenSSL 3. |
| `ubuntu:24.04` (status quo) | 2.39 | `libssl.so.3` | current, worst reach |

So the brief's option ranking inverts once the OpenSSL ABI is measured: **option 1 as written is
unshippable, and the reachable floor is 2.34 (x86_64) / 2.35 (aarch64), not 2.28.**

Going below 2.34 would require vendoring/statically linking OpenSSL, which changes the dependency
graph and means the product stops receiving distro OpenSSL security updates — a product decision,
not a build-container choice, so it is out of scope for this lane and recorded as a follow-up.

## Chosen plan

- **x86_64** → build in `almalinux:9` (glibc 2.34, `libssl.so.3`).
- **aarch64** → build in `ubuntu:22.04` with arm64 multiarch cross toolchain (glibc 2.35,
  `libssl.so.3`), replacing `cross` on the release path.
- Add a CI gate that reads each Linux artifact's max `GLIBC_*` and fails if it exceeds a declared
  per-target floor, proven in BOTH directions.

## MEASUREMENT 3 — floors, built on hetzner, same instrument each time

Every build is `cargo build --release --target <triple> -p wcore-cli`, rust 1.95.0, differing
ONLY by container image. Measurements written to a file on the host and read with the Read tool,
never through a shell pipeline (§3b).

| Build | Image | Target | MAX GLIBC | `NEEDED` openssl |
|---|---|---|---|---|
| baseline | `ubuntu:24.04` | x86_64 | **2.39** | `libssl.so.3` |
| baseline | `ubuntu:24.04` multiarch | aarch64 | **2.39** | `libssl.so.3` |
| candidate | `almalinux:9` | x86_64 | **2.34** | `libssl.so.3` |

Build cost: 6m14s (baseline x86_64) vs 6m06s (almalinux:9 x86_64) — the container change is
**cost-neutral**, and it removes `cargo install cross --git` (a from-source install) from the
aarch64 leg.

### The lexicographic bug, caught in real data rather than only in the self-test

The `almalinux:9` binary's true max is `GLIBC_2.34`. A plain `sort -u | tail -1` over the same
symbol list returns **`GLIBC_2.9`**, because "9" sorts above "3" as text. So the natural one-line
floor check would have reported this binary as needing 2.9 and would have passed *any* floor. The
gate compares integer tuples and its self-test asserts this exact divergence.

Note the baseline aarch64 binary's lexicographic max happens to be correct (2.39), because its
symbol set contains no `2.9`. That is why the bug survives casual testing: it is data-dependent.

## Status

- [x] Read LANE-BRIEF, MILESTONE-SHIP, release.yml, Cross.toml
- [x] P1 verified from source
- [x] P2/P3 measured; P3 refuted my own counter-hypothesis
- [x] OpenSSL 3 ABI constraint discovered — inverts the brief's option ranking
- [x] x86_64 floor lowered 2.39 → 2.34 and measured
- [x] Gate written, self-test 4/4, integrated into release.yml
- [ ] aarch64 candidate built via the CHECKED-IN script and measured
- [ ] Live executability matrix (both directions) on real distros
