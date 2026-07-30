# 27-C5 aarch64 — LANE SUMMARY

**Lane** `lane/27c5-aarch64` · **base** `b2ddf113` · evidence in
`evidence/27-c5-aarch64/`

**Verdict: the aarch64 gap is closed for two of the three aarch64 targets, and the third is
proven unmeasurable on the hardware this programme can reach.** The RC-blocking sentence
— *"an aarch64 binary is on the download page and has never been executed"* — is **no longer
true for `aarch64-unknown-linux-gnu`**. It remains true for `aarch64-pc-windows-msvc`, and I
could not change that; I proved why rather than working around it.

---

## 1. Per-target result

| Target | Grade | Counts | Hardware it ran on |
|---|---|---|---|
| `aarch64-apple-darwin` (v0.12.25) | **8 PASS / 1 RED** | `PASS=8 FAIL=1 NOT_MEASURED=0`, total 9 | This Mac — native Apple Silicon, macOS 26.3 |
| `aarch64-unknown-linux-gnu` (v0.12.25) | **8 PASS / 1 RED** ← **NEW** | `PASS=8 FAIL=1 NOT_MEASURED=0`, total 9 | linux/arm64 guest on this Mac's Apple Silicon |
| `aarch64-unknown-linux-gnu` (candidate `b7560018`) | **9 PASS / 0 RED** ← **NEW** | `PASS=9 FAIL=0 NOT_MEASURED=0`, total 9 | same arm64 guest |
| `aarch64-pc-windows-msvc` (v0.12.25) | **NOT MEASURED** | no run — binary cannot launch | no Windows-on-ARM host exists |

The single RED on the two v0.12.25 rows is the corpus's **deliberate falsifier**
(`ollama_hint_is_honest`), which is RED on all five packaged artifacts ever smoked. Grades are
**byte-identical to the three already on record** for macOS aarch64, Linux x86_64 and Windows
x86_64 — five platforms, one grade.

**Unrun cells: zero inside every run.** Every run reports `total=9` with `NOT_MEASURED=0`. The
only NOT MEASURED in this lane is a whole target, and it is named as such.

## 2. My brief contained one false claim — corrected

The brief implied `aarch64-apple-darwin` was unmeasured and should be smoked here. **It was
already measured.** `27-GAPS-SUMMARY.md`'s C5 table records macOS aarch64 at 8 PASS / 1 RED
and `macos-aarch64-v0.12.25.json` was already on disk. The two genuinely unmeasured targets
were `aarch64-unknown-linux-gnu` and `aarch64-pc-windows-msvc`. I re-ran macOS anyway, but as
a **live known-positive for the harness on this hardware**, not as gap closure — and it
reproduced the recorded grade exactly.

## 3. Both directions of the control (LANE-BRIEF §3b-iii)

The falsifier is RED on every packaged artifact, so on aarch64 alone it would have been
indistinguishable from a permanently-red gate — the defect that mis-graded `22-C3`. Closed
twice over, both on aarch64 hardware:

| | Falsifier | Corpus |
|---|---|---|
| **Can fail** — `v0.12.25` packaged, aarch64 Linux | **RED** | 8 PASS / 1 RED |
| **Can pass** — candidate `b7560018`, same guest | **GREEN** | **9 PASS / 0 RED**, run with **no `--expect-red` allowance at all** |

Plus a constructed control on the released binary itself: no credential → output carries
`No API key found` **and** `requires an API key` → matcher renders FAIL; credential present →
neither string → matcher renders **PASS**. Same binary, one capture.

## 4. Why the Windows aarch64 leg is NOT MEASURED — proven, not asserted

Hosts probed by measurement: `SeanD@seandesktop` = i9-13900KF / **AMD64** / Windows 11 Pro;
`hetzner-dsm` = x86_64 Linux; this Mac = **no VM software installed** (no UTM, Parallels,
VMware, VirtualBox, qemu). `seandesktop` connected fine — **this is not an access blocker.**

Rather than assert the impossibility I measured it on real Windows. Both archives were copied
to `D:\lane-27c5` (never `C:\` root), digest-verified on the Windows side, launched by the
same script in the same session:

```
PE_MACHINE_aarch64=0xAA64
LAUNCH_EXCEPTION_aarch64=This version of %1 is not compatible with the version of
                         Windows you're running.        <- ERROR_BAD_EXE_FORMAT
EXITCODE_aarch64=NOLAUNCH                               <- process never started

PE_MACHINE_x86_64=0x8664
EXITCODE_x86_64=0
OUTPUT_x86_64=wayland-core 0.12.25                      <- KNOWN-POSITIVE, same session
WLDONE
```

**The x86_64 leg is what makes this a measurement rather than a self-passing negative**
(§3b-i): it proves path, launcher, redirection and transport were all alive, so the ARM64
failure is the architecture alone. No result depended on an exit status crossing
ssh+PowerShell — the status file was read back by a **separate** ssh call keyed on `WLDONE`.

Closing it needs the Windows-on-ARM runner `release.yml:515-516` already calls "parked".

## 5. Is a virtualized arm64 guest legitimate? I measured it rather than claiming it

`host-arm64-guest-nativeness.txt`: `uname -m`=`aarch64`, kernel `6.12.76-linuxkit`;
**`/proc/sys/fs/binfmt_misc` is not even mounted**, so no cross-architecture interpreter of
any kind is registered and nothing is translating instructions; `/proc/cpuinfo` reports
**`CPU implementer: 0x61` (Apple)** with `paca`/`pacg`, `jscvt`, `bf16`, `i8mm`, `ebf16`.
Apple ARM cores are executing the release binary's ARM64 instructions directly.

I label this **real aarch64 hardware, virtualized guest OS — not bare metal**, and it is
emphatically **not** the qemu-user path `release.yml:565` deleted. The brief permitted qemu
only with a prominent disclaimer; no qemu was used.

## 6. Findings

**HIGH-ish, documentation/claim defect — `release.yml:692-693` overstates npm gating.** The
npm-publish job is gated on `post-tag-smoke` with the comment *"npm only ever serves a binary
that already ran `--version` on its native OS."* **That is false for two of the six platform
packages.** `post-tag-smoke` header-verifies aarch64-linux (ELF `e_machine`) and
aarch64-windows (PE `Machine`) and never executes them (`release.yml:624-684`). The
`aarch64-linux` half of that overstatement is now retired by measurement — the artifact does
run. The `aarch64-windows` half stands. **Recommend correcting the comment**; I did not touch
it, because `release.yml` is release-coordination surface and outside my declared paths.

**A finding I raised and then refuted with a control.** The aarch64 binary will not load on
Ubuntu 22.04 (`GLIBC_2.38`/`GLIBC_2.39` not found). My first read was an aarch64-only
regression. **False** — `readelf -V` over *both* published Linux binaries in one capture shows
the identical `GLIBC_2.39` maximum. What survives is architecture-neutral and out of scope:
**every Linux release artifact requires glibc ≥ 2.39** (Ubuntu 24.04+), so Ubuntu 22.04 LTS —
supported until 2027 — can run neither. Instrument controls in the same capture:
known-positive `GLIBC_2.17` → 91 hits, known-negative `GLIBC_9.99` → 0.

**Not a defect, worth recording as good behaviour.** A release build with no attributable
source identity refuses to build (`crates/wcore-cli/build.rs:11`), and its error message names
its own remedy precisely enough to act on without reading the source.

## 7. Deviations and mistakes

- Two of my four candidate-build attempts failed **on my errors**, not the repo's: I pinned
  `cross` 0.2.5 (whose aarch64 image has OpenSSL 1.0.2) instead of CI's git-main `cross`; and
  a shell-quoting bug redirected a log to a literally-quoted filename. The repo's cross setup
  is fine. Recorded so nobody concludes otherwise.
- The candidate build was **beyond my brief** (the brief asked only for packaged smokes). I
  did it because the falsifier's pass direction could not otherwise be shown on aarch64.
- `CARGO_BUILD_JOBS=10` throughout, per the concurrency instruction.

## 8. What I did NOT do

- Did **not** smoke `aarch64-pc-windows-msvc`. No host exists; reported NOT MEASURED.
- Did **not** produce a candidate for `aarch64-apple-darwin` or `aarch64-pc-windows-msvc`
  (would need a Mac workspace build, forbidden, and a WoA host, absent).
- Did **not** use qemu anywhere.
- Did **not** edit `release.yml`, `CRITERIA-STATUS.md` or `CRITERIA-GAP-LEDGER.md` — grading
  and release surface belong to the orchestrator. The ledger row is now stale in this lane's
  favour and needs a re-grade it is not mine to write.
- Did **not** push to integration, open a PR, tag, or close anything.

## 9. Proposed row text (for the orchestrator, not applied by me)

> `27-C5` — **PARTIAL ↑↑.** Five packaged smokes now run on real hardware across three OS
> families and two architectures — macOS aarch64, Linux x86_64, Linux **aarch64**, Windows
> x86_64, all 8 PASS / 1 RED byte-identical — plus a **candidate** aarch64 Linux build at
> 9 PASS / 0 RED. Of the six shipped targets, **five have now executed**. One remains
> **NOT MEASURED**: `aarch64-pc-windows-msvc`, blocked on a Windows-on-ARM host, with the
> impossibility measured on real Windows against a live x86_64 control.
