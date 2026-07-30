# SUMMARY — lane/glibc-reach

**Verdict: goal ACHIEVED, and the brief's recommended option was refuted by measurement.**

Both Linux release targets drop from a **GLIBC_2.39** floor to **GLIBC_2.34**, measured on real
binaries, with the binary live-executed on five distro families that could not run it before, and
a CI gate that fails the release if the floor ever rises again — proven able to both pass and fail.

Branch `lane/glibc-reach`, base `5cd37f79`. All builds on `hetzner-dsm`.

---

## 1. The option I chose, and why the brief's ranking inverts

The brief ranked `rockylinux:8` (glibc 2.28) as "best reach; covers everything listed above".
**That option is unshippable, and building it would have made reach worse.**

The reason is a second ABI constraint the brief did not account for. The shipped binary carries:

```
NEEDED  libssl.so.3
NEEDED  libcrypto.so.3
```

`openssl-sys` and `native-tls` are in `Cargo.lock`, so the binary is bound to **OpenSSL 3**. Every
base old enough to give glibc 2.28 — `rockylinux:8`, `almalinux:8`, `manylinux_2_28` — ships
OpenSSL **1.1**, and would emit a binary needing `libssl.so.1.1`. That soname does not exist on
Ubuntu 22.04, Debian 12 or RHEL 9. It trades a glibc break for an OpenSSL break **on precisely the
distros the lane exists to reach**.

So the floor is squeezed from both sides, and the reachable minimum is the lowest glibc that still
carries OpenSSL 3:

| Base | glibc | OpenSSL | Verdict |
|---|---|---|---|
| `rockylinux:8` / `manylinux_2_28` | 2.28 | `libssl.so.1.1` | **rejected** — breaks Ubuntu 22.04, Debian 12, RHEL 9 |
| `debian:11` | 2.31 | `libssl.so.1.1` | rejected, same reason |
| **`almalinux:9`** (chosen, x86_64) | **2.34** | `libssl.so.3` | lowest glibc that still has OpenSSL 3; supported to 2032 |
| **`ubuntu:22.04`** (chosen, aarch64) | 2.35 sysroot | `libssl.so.3` | lowest OpenSSL-3 base with working arm64 multiarch |
| `ubuntu:24.04` (status quo) | 2.39 | `libssl.so.3` | what we ship today |

Option 3 (static musl) was not pursued: it does not avoid the problem, because the arm64 C
dependencies (`libdbus`, `libseccomp`, `libasound`, OpenSSL) still need a sysroot, and `aws-lc-sys`
is in the graph. Going below 2.34 requires **vendoring OpenSSL**, which forfeits distro security
updates for TLS — a product decision, not a build-container choice. Recorded as a follow-up, not
taken unilaterally.

## 2. Measured before/after — BOTH Linux targets

Same command each time (`cargo build --release --target <triple> -p wcore-cli`, rust 1.95.0),
differing **only** by container image. Raw `readelf` captures in `evidence/*.floor.txt`.

| Target | Before | After | Built by |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | **GLIBC_2.39** | **GLIBC_2.34** | `almalinux:9` |
| `aarch64-unknown-linux-gnu` | **GLIBC_2.39** | **GLIBC_2.34** | `ubuntu:22.04` multiarch |

The aarch64 "before" is worth stating plainly: **the `cross` leg was not exempt.** I predicted it
would already be low, since a `cross` build links against the container sysroot rather than the
runner. Measured instead — `ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main` is itself Ubuntu 24.04
with a **glibc 2.39 aarch64 sysroot**. My counter-hypothesis was wrong and the brief was right.

aarch64 achieves 2.34 even though its sysroot is 2.35, because the binary references no 2.35
symbol. The declared floor is set to the **achieved** 2.34, not the permitted 2.35, so a future
change that pulls in a 2.35 symbol reddens the gate instead of silently dropping RHEL 9.

## 3. Distros that newly work — live-executed, both directions

`evidence/liveexec-matrix.txt`. NEW = `almalinux:9` build. OLD = what we ship today.

| Distro | glibc | NEW | OLD |
|---|---|---|---|
| Ubuntu 22.04 LTS | 2.35 | `wayland-core 0.12.25`, rc=0 | `GLIBC_2.39' not found` |
| Debian 12 bookworm | 2.36 | rc=0 | `GLIBC_2.39' not found` |
| Rocky Linux 9 | 2.34 | rc=0 | `GLIBC_2.39' not found` |
| AlmaLinux 9 | 2.34 | rc=0 | `GLIBC_2.39' not found` |
| Amazon Linux 2023 | 2.34 | rc=0 | `GLIBC_2.39' not found` |
| Ubuntu 24.04 | 2.39 | rc=0 | **rc=0** |

The last row is the control that makes the rest mean anything: the OLD binary **does** run on
24.04, so the five failures above are genuine glibc-floor failures and not a corrupt artifact. By
extension RHEL 9 and CentOS Stream 9 (both 2.34) are covered.

## 4. The gate, in both directions

**Build-time, symbol-level** — `.github/scripts/check_glibc_floor.py`, run per Linux target against
a declared floor, self-test first.

The natural one-liner is wrong and this is not hypothetical. `sort -u | tail -1` is
**lexicographic**, so it ranks `2.9` above `2.39`. On the real AlmaLinux binary the true max is
`GLIBC_2.34` while a plain `sort -u` returns **`GLIBC_2.9`** — that gate would pass every floor
forever. The script compares integer tuples, and the self-test's third assertion asserts precisely
this divergence, so the self-test cannot pass on the broken implementation. Note the baseline
aarch64 binary's lexicographic max happens to be *correct*, which is why the bug survives casual
testing: it is data-dependent.

`evidence/gate-proof.txt`, six cases against real binaries:

| # | Case | Result |
|---|---|---|
| 0 | self-test | 4/4 |
| 1 | **can it pass** — new x86_64 (2.34) vs 2.34 floor | rc=0 |
| 2 | **can it pass** — new aarch64 (2.34) vs 2.34 floor | rc=0 |
| 3 | **can it fail** — planted: real 2.39 x86_64 binary vs 2.34 | rc=1, names 2.38, 2.39 |
| 4 | **can it fail** — planted: real 2.39 aarch64 binary vs 2.34 | rc=1 |
| 5 | vacuity — a non-ELF file | rc=1, never a silent pass |
| 6 | **control** — the SAME 2.39 binary vs a 2.39 floor | rc=0 |

Case 6 is what rules out a permanently-red instrument (§3b-iii): it proves cases 3–4 failed on the
**floor**, not because the file was unreadable. Case 5 applies §3b-i to the gate itself — zero
symbols found is treated as a broken measurement, because a wrong path, a non-ELF file and a dead
`readelf` all produce "no offending symbols" for free.

**Post-tag, executable-level** — a new step runs the published artifact inside `rockylinux:9`
(exactly at the 2.34 floor). The pre-existing `--version` smoke runs on `ubuntu-latest` (24.04) and
therefore *structurally cannot* detect a floor regression. Proven with the exact step body
(`evidence/ci-step-sim.txt`): new artifact → `STEP_RESULT=PASS`; today's artifact →
`GLIBC_2.38/2.39 not found`, `STEP_RESULT=FAIL`.

## 5. What it cost

- **Build time: neutral.** 6m06s (`almalinux:9`) vs 6m14s (`ubuntu:24.04`).
- **`cross` leaves the release path.** This *removes* `cargo install cross --git` — a from-source
  install of a tool from a git default branch — from every release. `ci.yml` still uses `cross`, so
  `Cross.toml` is untouched.
- **`mold` and the runner-side apt install are dropped** from the Linux legs. They were already
  inert: nothing in `.cargo/config.toml` ever selected mold as the linker, and a runner-side sysroot
  is invisible to a compiler running in a container.
- **No feature was disabled.** `voice`/`libasound`, `libseccomp`, `libdbus` and OpenSSL 3 are all
  still linked; the `NEEDED` set is byte-identical before and after.
- **No dependency change.** `cargo metadata --locked` → **rc=0** on a clean tree
  (`evidence/locked-check.txt`).
- Residual risk: `ubuntu:22.04` reaches EOL April 2027, and its 2.35 sysroot is one version above
  the 2.34 we promise for aarch64. Both are one-line changes and the gate makes drift loud.

## 6. A CI-breaking defect found only by running it

`crates/wcore-cli/build.rs:11` refuses a release build with no attributable source identity, and
falls back to `git rev-parse HEAD`. **That fallback cannot fire inside the container**: it runs as
root over a workspace owned by another uid, and git refuses with "detected dubious ownership". Both
targets failed at first attempt. The build script now resolves the SHA on the host and injects
`WAYLAND_BUILD_SOURCE_SHA`, validated as 40 lowercase hex so an empty value fails loudly rather
than producing an unattributable release binary.

Had this lane delivered only the YAML edit, **the first tagged release would have failed.** This is
the concrete argument for the standing "live testing outranks green code" rule.

## 7. Deviations, and what I did NOT do

- **Deviated from the brief's recommended option 1**, on measured evidence (§1). The brief invited
  this: "pick one, justify it, and say what it costs."
- **Declared floor 2.34 for aarch64 rather than 2.35**, because 2.34 is what is achieved.
- Added the post-tag executable check, which the brief did not ask for; it is the only guard that
  catches a floor regression at the level a user experiences.
- **NOT done:** no PR, no tag, no push to `main` or to the integration branch, no release workflow
  triggered. Nothing was verified by an actual GitHub Actions run — every proof is a local
  reproduction of the same commands on hetzner. **The first real tag remains the true test.**
- **NOT done:** OpenSSL vendoring, which is what a sub-2.34 floor would require. Product decision.
- **NOT done:** `aarch64` was never *executed*, only symbol-measured — hetzner is amd64 and no
  aarch64 host was available to this lane. Its 2.34 figure is a `readelf` measurement, and the
  existing `27c5-aarch64` lane owns real ARM execution.
- **Instrument repair (§6b-ii):** my first completion check polled for the evidence file's
  *existence*, but the script creates it up-front and streams — a self-passing check. Repaired
  mid-run to poll process liveness and line count. My first build script also fused measurement
  into the build and lost a measurement to a quoting fault; measurement was split into a standalone
  tool, which is what produced every number above.
