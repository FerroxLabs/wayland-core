# UAT-TUI-WINDOWS — running NOTES (append-only, committed continuously)

Lane: `uat-tui-windows`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-uat-tui-windows`,
branch `lane/uat-tui-windows`, base `e9bed1af931f02aea094469d44eed291af0c4c96`.

Goal: user-acceptance-test the TUI and CLI as a *product* on real Windows, by launching and
driving the shipped binary. Not a test-suite run.

---

## Session start — environment facts established (each measured, not assumed)

| Fact | Value | How measured |
|---|---|---|
| Worktree toplevel | `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-uat-tui-windows` | `/usr/bin/git rev-parse --show-toplevel` |
| Worktree HEAD | `e9bed1af931f02aea094469d44eed291af0c4c96` | `/usr/bin/git rev-parse HEAD` |
| Integration head on GitHub | `e9bed1af9...` (identical) | `/usr/bin/git ls-remote gh refs/heads/plan/f20-unified-audit-repair` **and** independently from the Windows host — two hosts agree |
| Windows host | `SeanDesktop`, reachable | `ssh -o BatchMode=yes SeanD@seandesktop 'hostname'` → `SeanDesktop`, rc=0 |
| Windows shell | **PowerShell 5.1.26100.8875** | `$PSVersionTable.PSVersion.ToString()` |
| OS | Windows 11 Pro, build 26200 | `Get-CimInstance Win32_OperatingSystem` |
| CPU / RAM | i9-13900KF, 32 logical cores, 127.8 GB | `Get-CimInstance Win32_Processor` / `Win32_ComputerSystem` |
| Rust toolchain on host | rustc 1.95.0 (59807616e 2026-04-14), cargo 1.95.0 | `rustc --version` on host |
| Disk | C: 650.9 GB free / D: 5412.8 GB free / E: 1860.8 GB free | `Get-PSDrive -PSProvider FileSystem` |
| **LongPathsEnabled** | **`1`** (registry `HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem`) | `Get-ItemProperty` |
| Secret available | `~/.wayland-secrets/flux.env` contains exactly one key, `FLUX_API_KEY` (1 line, 72 bytes) | `grep -oE` for key NAMES only — value never read into context |

Working directory chosen: **`D:\lane-uat-tui-win\`** (brief §6: never `C:\` root).
`C:\actions-runner-{core,ferrox,wayland}` confirmed present on the host and **will not be touched**.

### `LongPathsEnabled=1` is a premise that changes the MAX_PATH leg

The brief points at a real `backup create`/`restore` `os error 3` defect past `MAX_PATH`. This host has
the long-path registry key **enabled**, which means a naive "it works here" result would NOT
generalise to a default Windows box, and equally a failure here is *worse* than it looks. I will
record the registry state alongside the result rather than reporting a bare pass/fail.

---

## Instrument defects already hit this session (before any product testing)

1. **`rtk` rewrote `git log` on my very first command.** `git log --oneline -3` reported top commit
   `9fc9a2ff`; `/usr/bin/git rev-parse HEAD` reported `e9bed1af`. The proxied log had **dropped the
   merge commit** that is actually HEAD — exactly the failure documented in LANE-BRIEF §3b. Every
   number and SHA in this report comes from an absolute-path tool, redirected to a file where it is
   load-bearing.
2. `/usr/bin/cat` does not exist on this Mac (it is `/bin/cat`). Cosmetic, noted so a re-runner does
   not trip on it.

## Method commitments (written before results exist, so they cannot be retrofitted)

- **Exit status will not cross ssh.** Per §6b-ii, every non-zero collapses to `1` over
  ssh+PowerShell. All Windows legs write `WLRC=<code>` first and `WLDONE` last into a per-leg status
  file; a **separate** ssh call reads it back. Three-state grading: no marker = incomplete, marker
  without status = UNREADABLE, both = true code.
- **Binary identity gets asserted before any conclusion is drawn from it**: path, sha256,
  `--version`, PE machine type. An artifact that was silently the wrong build has bitten this
  project before.
- **Both directions on every gate.** For the "binary exists and is executable" assertion I will
  prove it *fires* by pointing it at a path that does not exist.
- **A skip is not a pass.** Unrun cells get counted and reported as UNRUN with the reason.
- **The credential never lands on disk.** stdin-only transfer, never in argv, never echoed, swept
  for afterwards with an expected hit count of 0.

## Getting a REAL console for the TUI — the central problem, and how it was solved

ssh gives no console. A TUI with no console may exit 0 and read as a pass, which is precisely the
trap the brief names. Two facts shaped the approach:

- A prior lane (`.planning/evidence/windows-legs-sweep/NOTES.md:113`) measured that
  **`portable_pty`'s ConPTY backend does not surface the child's stdout**, which is why this repo's
  own `pty_capture.rs` is `#![cfg(unix)]`. So the repo's own harness cannot drive a Windows TUI.
- Its consequence, also measured there: with no terminal, `confirm.rs` denies confirmable tool
  calls on piped stdin, so **16 of 22 pairs did not run** on Windows.

I therefore drove the product through **pywinpty 3.0.5** (Python 3.12.10), which binds the same
Win32 ConPTY API Windows Terminal itself uses, and rendered the byte stream through **pyte** so the
capture is literally the screen a user would see. Driver: `D:\lane-uat-tui-win\drive_tui.py`.

### The driver was DEAD THREE TIMES and every failure looked like a clean run

This is the §3b-i shape, hit repeatedly against my own instrument. Recorded because each version
would have produced a confident, false product finding:

| Ver | Defect | What it would have reported |
|---|---|---|
| v1 | passed a **list** argv; pywinpty wants a command string | "TUI renders nothing on Windows" — 23 bytes, blank screen, no error |
| v2 | read in a **background thread**; pywinpty is not thread-safe here, the reader died after the first chunk and the driver **swallowed the exception** | same false blank screen, 23 bytes |
| v3 | read loop deadline was 2.0 s, but **ConPTY stalls ~3.0 s** before delivering any child output | 28 bytes, blank screen — a *timing* artifact indistinguishable from a broken product |

The v3 stall is worth stating precisely, measured by timestamped reads: bytes arrive at
`0.00s → 4`, `0.01s → 23`, `0.01s → 28`, then **nothing until `3.03s`**, then `33 → 57 → 59 → 102 →
104`, EOF at `3.04s`. **Any Windows ConPTY harness with a settle under ~3.5 s silently reports an
empty screen.** All three defects were caught by the known-positive control, none by inspection.
Per §6b-ii the instrument was repaired in-lane rather than noted, and the repair kept its history.

### Instrument liveness, both directions (run before ANY product measurement is believed)

- **Known-positive:** `cmd.exe /c echo KNOWN_POSITIVE_MARKER & ver` → `WLRC=0`, `WLBYTES=104`,
  rendered screen contains `KNOWN_POSITIVE_MARKER` and `Microsoft Windows [Version 10.0.26200.8875]`.
  The instrument can pass.
- **Known-negative:** same driver aimed at `D:\lane-uat-tui-win\this-binary-does-not-exist.exe` →
  `FileNotFoundError: The command was not found or was not executable`, **no `.status` file written**,
  so the three-state grade is *incomplete*, not *pass*. The instrument can fail.

## Instrument defects hit this session, continued

3. **cmd/PowerShell quote-mangling is real and I hit it.** An inline
   `python -c "import winpty; print(\"x\")"` sent over ssh arrived at Python as
   `print(" pywinpty\,` → `SyntaxError: unterminated string literal`. **Every** script and spec is
   now written locally and `scp`'d as a file; the only things crossing the boundary inline are
   unquoted paths. JSON spec files exist for this reason.
4. **zsh ate an unquoted `--include=*.rs` glob** — and my known-positive control returned **0**,
   which would have "confirmed" that `FLUX_API_KEY` does not appear in the codebase. Re-run quoted:
   `FLUX_API_KEY` appears in real code (`wcore-providers/src/fingerprint.rs:125` maps it to
   `flux-router`), and the control returns **53** files for `ANTHROPIC_API_KEY`. Exactly the
   §3b-i failure the brief predicts, reproduced on the first attempt.

## Status log

- [t0] Environment established (table above). Windows clone of `plan/f20-unified-audit-repair`
  into `D:\lane-uat-tui-win\repo`; **CLONE_HEAD asserted = `e9bed1af931f02aea094469d44eed291af0c4c96`**,
  identical to my Mac worktree HEAD and to `git ls-remote` from both hosts independently.
- [t1] Release build of `-p wcore-cli` launched detached on the Windows host, communicating only
  through `D:\lane-uat-tui-win\status\build.status` (`WLRC=` then `WLDONE`).
- [t2] ConPTY driver built, killed three times by its own defects, repaired, and proven alive in
  both directions. Product measurement can now begin.
- [t3] Live-turn recipe located from a prior lane (`.planning/evidence/e2e-product-smoke/journey2.sh`):
  provider `flux-router`, model `flux-standard`, `base_url = https://api.fluxrouter.ai/v1`,
  config at `$WAYLAND_HOME/config.toml`, plus `WAYLAND_VAULT_PASSPHRASE`. Key comes from
  `FLUX_API_KEY` (`wcore-config/src/config.rs:2965`).
