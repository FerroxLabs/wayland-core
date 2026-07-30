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

### T2 — binaries, both attributable by the product's OWN self-report
| Platform | Path | sha256 | `--build-info` source |
|---|---|---|---|
| Linux x86_64 | `/root/wayland-uat-tui-unix/target/release/wayland-core` | `fae1287a…d059a` | **`e9bed1af`** (= integration base) |
| macOS arm64 | CI artifact `8752771508` | `e5803944…c662b` | **`a903142b`** (ancestor, 33 behind) |

`--build-info` prints the embedded source SHA, so provenance is not an inference.

### T3 — TUI renders on BOTH platforms, but boot time differs 5.5x
Measured at 1s granularity, pristine HOME, all provider keys unset, real tmux pty
(pty proven: isatty False outside / True inside):
- macOS: splash 1s → **onboarding at 2s**
- Linux: splash 1s → **onboarding at 11s** (hetzner shared with other lanes; caveat noted)

Onboarding surface itself is good: ASCII logo, "connect a provider to begin", paste box,
and three routes (API key / Ollama local / skip). Instrument alive on both (needle matched).

### T4 — HEADLESS CLI CANNOT COMPLETE A TURN OUT OF THE BOX (both platforms)
`wayland-core -p flux-router -m flux-auto --no-tui "<prompt>"`:

| Config | Platform | rc | What the user gets |
|---|---|---|---|
| pristine HOME | macOS | **1** | `error: Session persistence authority unavailable: … no OS keyring was usable and no encrypted credentials vault is unlocked` |
| pristine HOME | Linux | **1** | identical |
| real HOME (`backend="plaintext"`) | macOS | **1** | `Error: storage.credentials.backend is set to "plaintext", which cannot hold the confidential key that durable session recovery requires` |
| + `WAYLAND_VAULT_PASSPHRASE` | macOS | **0** | `* WAYLAND_UAT_OK` `[turns: 1 \| tokens: 8581 in / 8 out]` |
| + `WAYLAND_VAULT_PASSPHRASE` | Linux | **0** | `* WAYLAND_UAT_OK` `[turns: 1 \| tokens: 2563 in / 8 out]` |

So the product **works**, but only after the user solves a credential-vault problem it
raises at them on the way in. On a headless Linux server — the canonical deployment — there
is no OS keyring, so this is the DEFAULT experience, not an edge case.

Arm verified per §3b-ii by reading the product's own output, not from my env: the capture
contains `flux-router/flux-auto` and 5× `api.fluxrouter.ai`, and **zero** `api.anthropic.com`
— despite `/root/.wayland/.env` existing on hetzner (checked and reported by the runner).

Secret discipline: key injected on stdin only, never argv/disk. Every capture swept —
`SECRET_LEAK_HITS=0` with `SECRET_SWEEP_KNOWN_POSITIVE=1` proving the sweep was alive.

### T5 — instrument defects found and REPAIRED in-lane (not just noted, per §6b-ii)
1. pty probe redirected its own stdout to a file → reported `ISATTY=False` inside a real pty.
   Repaired to read the pane back with `capture-pane`. It **refused to judge** rather than
   passing, which is the harness working correctly.
2. `pgrep -f <basename>` matched **17** unrelated processes (the checkout is named
   `waylandcore`); `pgrep -f <abspath>` then matched **the harness itself** (`--bin <path>`
   is in its own argv) and reported a false orphan. Repaired to exact `argv[0]` match.
3. `grep -c . file || echo 0` produced the literal two-line value `"0\n0"` — the exact trap
   in the brief. Replaced with `awk 'END{print NR+0}'`.
4. `pgrep -a` is GNU-only and is **silently ignored on BSD**, so macOS captures held bare
   PIDs with no command text — a detector that looks like it is reporting commands and is not.
5. macOS `env` parses options before operands, so `env HOME=x -u KEY` made `-u` the command:
   `env: -u: No such file or directory`, rc=127. GNU env tolerates it; BSD does not.

(appended as work proceeds)
