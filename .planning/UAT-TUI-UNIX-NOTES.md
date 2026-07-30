# UAT-TUI-UNIX — running notes (append-only, committed continuously)

Lane: `uat-tui-unix`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-uat-tui-unix`,
branch `lane/uat-tui-unix`, base integration `e9bed1af`.

Goal: user-acceptance-test the shipped TUI/CLI **by launching and driving it** on Linux and macOS.
Not compiling it. Not `cargo test`. Driving it.

## Ground rules I am operating under
- Never run cargo on the Mac (AGENTS.md / LANE-BRIEF §0). macOS binary must come from a CI artifact.
- Never print/echo/commit a secret. `~/.wayland-secrets/flux.env` is the FluxRouter burn key.
- Load-bearing numbers come from unproxied absolute-path tools, redirected to a FILE, read with
  the Read tool — `rtk` fabricates counts (LANE-BRIEF §3b).
- A TUI needs a pty. Headless ssh does not provide one; a TUI that fails to attach may exit 0.
- A skip is not a pass. Count and report every UNRUN cell.

## Timeline / measurements

### T0 — environment established
- Mac: `Darwin ... arm64` (T8142), macOS 25.3.0. This is the local host; the Mac is NOT an ssh target.
- hetzner-dsm: `Ubuntu-2404-noble-amd64-base`, `x86_64`, `/root` 995G free (41% used) — ample.
- Pre-existing Linux release binary: `/root/wayland/target/release/wayland-core`,
  98,434,848 bytes, mtime Jul 30 09:35. Identity (commit/sha256) NOT yet established — pending.
- Secrets present: `flux.env` (600), plus discord/matrix/slack env files.

### Open questions at T0
1. What commit is the hetzner release binary from? (brief claims `570056c1`) — must verify.
2. Can I obtain a genuine arm64 Mach-O macOS artifact? If not → macOS reported UNRUN, not substituted.
3. Does the TUI even attach under `script`/`tmux` on a headless box?

(appended as work proceeds)
