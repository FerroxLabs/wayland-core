# 27-C5 aarch64 gap closure — running NOTES

Lane `lane/27c5-aarch64`, base `b2ddf113`. Append-and-commit after every measurement
(LANE-BRIEF §6b-i). Nothing here is a conclusion until it names the capture it came from.

## 1. Premise check on my own brief — one claim was WRONG

My brief said: *"This Mac — Apple Silicon, i.e. real aarch64 macOS. `aarch64-apple-darwin`
can be smoke-tested natively HERE."* That reads as though `aarch64-apple-darwin` is one of
the two unmeasured targets. **It is not.**

`27-GAPS-SUMMARY.md` (C5 section, table) already records:

| Platform | Artifact | Result |
|---|---|---|
| macOS **aarch64** | `v0.12.25` tar.gz | 8 PASS / 1 RED |
| Linux x86_64 | `v0.12.25` tar.gz (digest-verified) | 8 PASS / 1 RED |
| Windows x86_64 | `v0.12.25` zip (digest-verified) | 8 PASS / 1 RED |

and states plainly: *"The two aarch64 targets are **NOT MEASURED** — no aarch64 Linux or
Windows-on-ARM host was available."* Evidence file `macos-aarch64-v0.12.25.json` exists on
disk in `evidence/27-gaps/c5-packaged-smoke/`.

**So the two NOT MEASURED targets are:**

1. `aarch64-unknown-linux-gnu`
2. `aarch64-pc-windows-msvc`

`aarch64-apple-darwin` is already measured. I will re-run it anyway as a **live control on my
own instrument** (it has a known expected result — 8 PASS / 1 RED — so it is a known-positive
for the harness), not as gap closure.

## 2. What `release.yml` actually does for the two unmeasured targets

Both are BUILT and PUBLISHED, and **neither is ever executed.**

- `.github/workflows/release.yml:624-645` — *"Verify Linux aarch64 binary file shape only
  (cannot execute on amd64 runner)"*. Checks ELF magic + `e_machine == 0x00b7` + size >= 1 MiB.
- `.github/workflows/release.yml:660-684` — *"Verify Windows aarch64 binary file shape only
  (cannot execute on amd64 runner)"*. Checks PE COFF `Machine == 0xAA64` + size >= 1 MiB.
- `release.yml:558-567` — the qemu execution path for aarch64-linux was **deliberately
  removed** ("qemu execution replaced by ELF-header check below") because the binary links
  against `libdbus` and a multiarch sysroot was judged not worth chasing. The comment says
  the binary "is shipped to real aarch64 hosts via M2.4's self-hosted runner (parked)."
- `release.yml:686-694` — **npm publish is gated on `post-tag-smoke`**, with the comment
  *"npm only ever serves a binary that already ran `--version` on its native OS."* That
  comment is **false for two of the six platform packages**: the aarch64-linux and
  aarch64-windows packages are published on the strength of a header check, never an
  execution. This is a documentation/claim defect independent of the criterion.

So the criterion's gap is real and the CI is honest about *what it checks* while the npm
gating comment overstates it.

## 3. Hosts probed (measured, not assumed)

| Host | How probed | Result |
|---|---|---|
| This Mac | `uname -m` | `arm64`, macOS 26.3 (25D125) — real aarch64 Darwin |
| `hetzner-dsm` | brief + prior lanes | x86_64 Linux — cannot execute aarch64 |
| `SeanD@seandesktop` | `$env:PROCESSOR_ARCHITECTURE`, `systeminfo` | `AMD64`, "x64-based PC" |
| Docker on this Mac | `docker version --format '{{.Server.Arch}} {{.Server.Os}}'` | **`arm64 linux`** |

`SeanD@seandesktop` reachable with `BatchMode=yes`, rc=0 — recording this because two prior
lanes wrongly reported it unreachable.

## 4. Plan, and the honest limit

- **`aarch64-unknown-linux-gnu` → MEASURABLE.** Docker on this Mac runs a **linux/arm64**
  guest. That is a Linux kernel executing ARM64 instructions on Apple Silicon under
  Virtualization.framework — **real aarch64 instruction execution, virtualized, NOT qemu-user
  emulation.** I will assert that distinction from inside the container rather than assert it
  from documentation, and I will label the result as "virtualized native aarch64" not "bare
  metal".
- **`aarch64-pc-windows-msvc` → probably NOT MEASURABLE.** No Windows-on-ARM host is known.
  An ARM64 PE cannot execute on x64 Windows and cannot execute on macOS. I will probe for a
  Windows-on-ARM VM on this Mac before declaring it, and if none exists I will report
  **NOT MEASURED with the reason**, per the brief's explicit instruction. I will NOT fake it.

## 5. Both-direction control (LANE-BRIEF §3b-iii)

The harness `scripts/f27-packaged-smoke.py` already carries its own falsifier:
`ollama_hint_is_honest` is **RED at v0.12.25** and **GREEN at lane HEAD** (recorded in
`27-GAPS-SUMMARY.md`). So:

- **Can it fail?** Yes — v0.12.25 reddens that probe on macOS, Linux and Windows x86_64.
- **Can it pass?** Yes — a lane-HEAD Linux x86_64 build greens it (`linux-x86_64-lane-HEAD-80c175e0.json`).

I must re-establish BOTH directions **on aarch64 itself**, not inherit them from x86_64 —
otherwise my aarch64 run has no control at all. Recorded as a TODO until done.

## 6. RESULT — `aarch64-unknown-linux-gnu` = **8 PASS / 1 RED / 0 NOT MEASURED**

**First execution of this artifact in the project's history.** Byte-identical grade to the
three platforms already recorded.

- Artifact: `wayland-core-v0.12.25-aarch64-unknown-linux-gnu.tar.gz`, published release
  `v0.12.25`.
- **Digest verified against the published `wayland-core-checksums.txt`:**
  `214bc7f87052b3bfb2e00cf1637223217e4c15e0ec84435b54918fdc23518380` — matches
  (`archive-digest-verification.txt`). The other two aarch64 archives were verified in the
  same capture and also match.
- Embedded build provenance read out of the binary itself: `source 61b79c4`.
- Host: **linux/arm64 guest on this Apple Silicon Mac** under Docker Desktop's
  Virtualization.framework, `ubuntu:24.04` + `libdbus-1-3` + `libseccomp2` + `python3 3.12.3`,
  glibc 2.39.
- Counts read back from `--status-file` by a **separate** call, not from an exit status:
  `WLRC=0 PASS=8 FAIL=1 NOT_MEASURED=0 UNEXPECTED_RED=` then `WLDONE`.
- `total=9` — **nine probes ran; zero were skipped.** No NOT-MEASURED cell inside the run.

### This is virtualization, not emulation — and I measured it rather than asserting it

`host-arm64-guest-nativeness.txt`:

- `uname -m` = `aarch64`, kernel `6.12.76-linuxkit`.
- **`/proc/sys/fs/binfmt_misc` is not even mounted** — there is therefore *no* registered
  cross-architecture interpreter of any kind in the guest. A qemu-user setup requires a
  binfmt handler; there is none, so nothing is translating instructions.
- `/proc/cpuinfo` reports **`CPU implementer: 0x61` (Apple)** with `paca`/`pacg` (ARMv8.3
  pointer authentication), `jscvt`, `bf16`, `i8mm`, `ebf16` — the Apple Silicon feature set.
  qemu's TCG CPU does not report implementer 0x61 or that feature vector.

So the ARM64 instructions in the release binary are being executed by Apple ARM cores.
**I am labelling this "real aarch64 hardware, virtualized guest OS" and not "bare metal",**
and it is emphatically not the qemu path that `release.yml` deleted.

## 7. A finding I nearly reported and then refuted with a control

The binary **fails to load on Ubuntu 22.04** (`linux-aarch64-ldd-ubuntu2204.txt`):

```
/bin-under-test/wayland-core: /lib/aarch64-linux-gnu/libc.so.6: version `GLIBC_2.38' not found
/bin-under-test/wayland-core: /lib/aarch64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found
libdbus-1.so.3 => not found
```

My first read was "the aarch64 build has a higher glibc floor than x86_64, and nobody noticed
because it is never executed." **That is false.** I ran the same `readelf -V` extraction over
BOTH published Linux binaries in one capture (`glibc-floor-differential.txt`): the maximum
required version is `GLIBC_2.39` for **both** `aarch64-unknown-linux-gnu` and
`x86_64-unknown-linux-gnu`. The floor is a project-wide property, not an aarch64 regression.

Instrument controls in that same capture: known-positive `GLIBC_2.17` → **91** hits;
known-negative `GLIBC_9.99` → **0** hits. The grep was alive in both directions.

**What survives as a real (non-aarch64) note:** every Linux release artifact requires
glibc >= 2.39, i.e. Ubuntu 24.04 / Debian 13 or newer. Ubuntu 22.04 LTS is supported until
2027 and cannot run either binary. Recording it; it is out of this lane's scope to fix.

## 8. Both directions of the gate, established ON aarch64 (LANE-BRIEF §3b-iii)

The corpus's falsifier `ollama_hint_is_honest` is RED on **every** packaged artifact ever
smoked. On aarch64 alone it would therefore be indistinguishable from a permanently-red gate
— the precise defect that mis-graded `22-C3`. So I constructed its pass state on the aarch64
binary itself (`probe9-both-directions-on-aarch64.txt`, one capture, same binary, same guest):

| Direction | State | Engine output | Matcher renders |
|---|---|---|---|
| **Can fail** | no credential | contains `No API key found` **and** `Provider 'anthropic' requires an API key` | **FAIL** |
| **Can pass** | credential present | contains **neither** string | **PASS** |

Plus, within the single graded run, the corpus produced **8 PASS and 1 FAIL on aarch64
hardware** — both grades reached on the target, in one run, with `total=9` and no skipped
cell.

Instrument known-positive: the same harness re-run on native aarch64 macOS reproduced the
grade already on record (8 PASS / 1 RED), so the harness is alive on this hardware rather
than inherited from x86_64.

## 9. RESULT — `aarch64-pc-windows-msvc` = **NOT MEASURED**, impossibility proven

No Windows-on-ARM host exists in this program's reach, and I measured that rather than
assuming it:

| Host | Probe | Result |
|---|---|---|
| `SeanD@seandesktop` | `Win32_Processor`, `PROCESSOR_ARCHITECTURE` | i9-13900KF, **AMD64**, Windows 11 Pro |
| `hetzner-dsm` | `uname -m` | x86_64 Linux |
| This Mac | `ls /Applications`, `which` | **no VM software at all** — no UTM, Parallels, VMware, VirtualBox, qemu |

`SeanD@seandesktop` connected fine with `BatchMode=yes`; this is not an access blocker.

Then, rather than assert "x64 Windows cannot execute an ARM64 PE", I ran it
(`windows-aarch64-NOT-MEASURED-proof.txt`). Both published archives were copied to
`D:\lane-27c5` (never `C:\` root), digest-verified **on the Windows side**, and launched by
the same script in the same session:

```
SHA256_aarch64=282cd3f309e17a62e712a97caab7b95050bc2fca9158b6b4a2b28a3f7d546c85   (matches published)
SHA256_x86_64 =e9cf6650a47a8f3340a25d720ab6ed032e1fa41da0d1171ccb87337fd542947a   (matches published)

PE_MACHINE_aarch64=0xAA64
LAUNCH_EXCEPTION_aarch64=This version of %1 is not compatible with the version of Windows
                          you're running.            <- ERROR_BAD_EXE_FORMAT
EXITCODE_aarch64=NOLAUNCH                            <- the process never started

PE_MACHINE_x86_64=0x8664
EXITCODE_x86_64=0
OUTPUT_x86_64=wayland-core 0.12.25                   <- KNOWN-POSITIVE, same script/session
WLDONE
```

**The x86_64 leg is the control that makes this a measurement and not a self-passing
negative.** It proves the path, the launcher, the redirection and the ssh transport were all
alive; the ARM64 failure is the architecture and nothing else.

No result here depends on an exit status crossing ssh+PowerShell — the status file was read
back by a **separate** ssh call and keyed on `WLDONE` (LANE-BRIEF §3.2).

**Recorded as NOT MEASURED. Not zero, not passing, not faked.** Closing it needs a
Windows-on-ARM host, which is the same "parked self-hosted ARM runner" `release.yml:515-516`
already names.

## 10. Out-of-scope extra attempted: an aarch64 Linux *candidate* build

C5 also distinguishes shipped release from candidate. Attempted a cross-build of the lane
candidate for `aarch64-unknown-linux-gnu` on hetzner.

**Attempt 1 FAILED, and the cause was mine, not the repo's.** I pinned `cross` 0.2.5, whose
default aarch64 image carries OpenSSL 1.0.2 (`cargo:version_number=1000207f`), and
`openssl-sys 0.9.116` aborts on it. `release.yml:136` installs `cross` from **git main**, whose
image has OpenSSL 1.1.1. Re-running matched to CI. Recording the misstep because a reader
should not conclude the repo's cross setup is broken — it is not; my invocation deviated
from CI.

## 11. Log

- Notes committed before any measurement, per §6b-i.
- `aarch64-unknown-linux-gnu` smoked: 8 PASS / 1 RED. Evidence committed.
- Both-direction control established on aarch64. Evidence committed.
- `aarch64-pc-windows-msvc`: NOT MEASURED, impossibility proven with a live control.
- Candidate cross-build attempt 1 failed (my pin); attempt 2 running.
</content>
</invoke>
