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

## Status log

- [t0] Environment established (table above). Windows clone of `plan/f20-unified-audit-repair`
  started into `D:\lane-uat-tui-win\repo`; SHA will be asserted against `e9bed1af9...` before any
  build output is trusted.
