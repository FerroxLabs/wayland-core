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

## 6. Log

- Notes committed before any measurement, per §6b-i.
</content>
</invoke>
