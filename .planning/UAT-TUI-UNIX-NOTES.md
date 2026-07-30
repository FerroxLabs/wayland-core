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

### T1 — artifact provenance established (both platforms)

**Linux (hetzner) pre-existing binary is NOT at integration base — attributability trap.**
- `/root/wayland` tree: HEAD `e9bed1af`, `git status --porcelain` = 0 lines (clean).
- `/root/wayland/target/release/wayland-core`: mtime `2026-07-30 09:35:35 UTC`.
- `e9bed1af` commit time: `2026-07-30 16:51:51 +0700` = **09:51:51 UTC**.
- The binary therefore predates its own checkout by **16 minutes**. A lane running that binary
  and reporting "UAT at e9bed1af" would be stating something false with a clean tree to back it up.
  → I am building my own at `e9bed1af` in `/root/wayland-uat-tui-unix` (worktree asserted
  `WT_SHA=e9bed1af...`, DIRTY=0).

**macOS arm64 artifact obtained — genuine, verified with a discriminating control.**
- Source: run `30524784173`, artifact `8752771508` `wayland-core-aarch64-apple-darwin`,
  head_sha `a903142b` ("merge(darwin-ci-selfhosted)"). `a903142b` **is an ancestor** of `e9bed1af`
  (`git merge-base --is-ancestor` rc=0), **33 commits behind**.
- `file` → `Mach-O 64-bit executable arm64`; `lipo -archs` → `arm64`.
- **Control**: `lipo -archs /bin/ls` → `x86_64 arm64e`. The instrument discriminates, so the
  subject's single-arch `arm64` is a real reading and not a dead tool returning one value.
- sha256 `e5803944aead3c987b8b158a71576bc2d0b49dde6e71a6463df20590089c662b`, 80,209,792 bytes.
- Runs on this Mac: `./wayland-core --version` → `wayland-core 0.12.25`, rc=0.
- Codesign: `adhoc, linker-signed`, `TeamIdentifier=not set`. No Developer ID. (Distribution finding.)
- No darwin artifact exists at `e9bed1af`: its CI run `30532433958` is `status=pending` with
  **zero jobs scheduled** — CI starvation. macOS leg is therefore at `a903142b`, disclosed, not
  substituted with the Linux result.

(appended as work proceeds)
