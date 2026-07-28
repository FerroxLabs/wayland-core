# C5 — packaged smokes, three native platforms

Phase 27's verdict read: "**Zero packaged smokes ran on zero platforms.** Every
Linux measurement in this phase came from a `cargo build --release` binary
inside a build tree. That is not a packaged artifact."

These runs are packaged artifacts. Each is a published release archive,
extracted and executed on the real operating system.

## Artifact provenance

Release `v0.12.25` of `FerroxLabs/wayland-core`, published 2026-07-13,
embedded source SHA `61b79c4` (reported by the binary's own `--build-info`
on all three platforms, so the three archives are the same source).

| Platform | Archive | SHA-256 | Verified where |
|---|---|---|---|
| macOS aarch64 | `wayland-core-v0.12.25-aarch64-apple-darwin.tar.gz` | (downloaded and run in place on the Mac) | Mac, Darwin arm64 |
| Linux x86_64 | `wayland-core-v0.12.25-x86_64-unknown-linux-gnu.tar.gz` | `c4fe22f4c5bb713181f38e6e308a56de0d033380bb1134d3834dd89d561c6751` | `hetzner-dsm`, digest re-computed remotely and compared |
| Windows x86_64 | `wayland-core-v0.12.25-x86_64-pc-windows-msvc.zip` | `e9cf6650a47a8f3340a25d720ab6ed032e1fa41da0d1171ccb87337fd542947a` | `seandesktop`, digest re-computed remotely and compared |

The digests were compared **because a transfer lied**. The first `scp` of the
Linux archive was killed by a `timeout` wrapper and left a 22,456,320-byte
prefix of a 29,802,427-byte file; the reported status was `rc=0`, because
`echo "scp_rc=$?"` after a piped command reads the pipe's last stage, not
`scp`. `tar` then failed with "unexpected EOF in archive". Every artifact
here is digest-verified at the destination for that reason.

## Results

Harness: `scripts/f27-packaged-smoke.py`. Nine probes, run under a throwaway
`WAYLAND_HOME` with 18 credential-bearing environment variables stripped, so
no probe can read the operator's real configuration or emit a secret.

| Probe | Criterion | macOS | Linux | Windows |
|---|---|---|---|---|
| `version_shape` | C5 | PASS | PASS | PASS |
| `build_provenance` | C5 | PASS | PASS | PASS |
| `profile_isolation_holds` | C5 | PASS | PASS | PASS |
| `builtin_generation_refuses_without_credential` | C3 | PASS | PASS | PASS |
| `builtin_generation_validates_input` | C3 | PASS | PASS | PASS |
| `web_fetch_refuses_without_credential` | C3 | PASS | PASS | PASS |
| `mcp_registry_discovery` | C3 | PASS | PASS | PASS |
| `host_protocol_honest_init_failure` | C1 | PASS | PASS | PASS |
| `ollama_hint_is_honest` | C2 | **FAIL** | **FAIL** | **FAIL** |

`8 PASS / 1 FAIL / 0 NOT MEASURED` on every platform, byte-identical grades.

## The red is deliberate and it is the point

`ollama_hint_is_honest` follows the engine's own printed remediation verbatim
and grades whether the instruction works. It does not. It is retained RED
rather than softened, for two reasons:

1. **A corpus in which every probe is green at every commit cannot fail, and
   therefore proves nothing.** This program has recorded eight instruments
   carrying the exact defect they hunt. This one demonstrably fails.
2. It is a real defect, fixed on this lane at `9fe6ad86`. The v0.12.25
   artifact predates that fix, so it stays red here forever — which is
   correct, because these files record what the *shipped* release does.

## How the Windows result was read

Exit status does not survive `ssh` to PowerShell: every non-zero collapses to
`1`. The harness writes `WLRC=` first and `WLDONE` last to a status file, and
a **separate** `ssh` call reads it back. `windows-x86_64-v0.12.25-status.txt`
is that read-back: `WLRC=0 / PASS=8 / FAIL=1 / WLDONE`. Both markers present,
so the run is complete rather than truncated.

## What this does NOT establish

- These are the **v0.12.25 release**, not the phase candidate. A packaged
  smoke of the current tree needs a release build per platform; only the
  Linux one is producible on hardware this lane can reach.
- aarch64 Linux and aarch64 Windows are **NOT MEASURED** here. No aarch64
  Linux or Windows-on-ARM host was available. They are not recorded as `0`
  and not recorded as passing.
- The probes exercise credential-absent behaviour. Nothing here exercises a
  *successful* generation, which needs a Flux credential (Sean-reserved).
