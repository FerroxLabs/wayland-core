# lane/boot-walk — NOTES (append-only, committed continuously)

Base: `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted via `git rev-parse HEAD` in the lane worktree).

## Instrument defects found in my own harness (repair in-lane, per LANE-BRIEF §6b-ii)

1. **`wc -l` fabricated `0` for a 12-line file.** First measurement of the lane:
   `/usr/bin/grep -rn WalkBuilder crates/ > /tmp/f.txt; wc -l < /tmp/f.txt` printed `0`; the
   Read tool on the same file shows 12 lines. Repair: every count in this lane comes from
   opening the file with the Read tool. Never `wc`, never a piped count.
2. **zsh ate an unquoted `--include=*.rs`** ("no matches found") — the brief predicted it.
   Repair: every glob quoted.

## Premise verification (brief's claims vs. HEAD c9ab048b)

### CONFIRMED
- `workspace_policy.rs:837-841` is verbatim `ignore::WalkBuilder::new(root).standard_filters(false)
  .hidden(false).follow_links(false).build()`. No prune. Comment at :828-836 states the no-prune
  choice is deliberate and load-bearing.
- Only three `WalkBuilder` construction sites exist in `crates/`: `wcore-repomap/src/lib.rs:52`,
  `wcore-repomap/src/scope.rs:209`, `wcore-tools/src/workspace_policy.rs:837`. Control: the same
  grep returns known positives (12 lines incl. the `use` statements), so it is alive.

### REFUTED / NOT SUPPORTED
- **"walk 1 = `wcore_repomap::scope::scope_files`" is not on the boot path.** `scope_files` has
  exactly two callers (`store.rs:518` in `IndexStore::refresh`, `store.rs:664` in `verify`), and
  `IndexStore::refresh` has exactly one production caller: `wcore-cli/src/index_cmd.rs:105`,
  the explicit `index` subcommand.
- `RepoMap::build` production callers are all on-demand: `tui/engine_bridge.rs:1384` (`/repomap`
  slash command, inside `spawn_blocking`), `tui/commands/at_ref_send.rs:352` (@symbol), and
  `wcore-tools/src/repomap.rs:95` (the RepoMap agent tool). None at boot.

### NEW, and the brief does not mention it: THE WALK IS POSTURE-DEPENDENT
`compute_secret_deny` only calls `project_committed_secrets` when `trust == Contained`
(`workspace_policy.rs:805`). `bootstrap.rs:2865-2875` selects `contained()` only when the session
is channel/remote, `Managed`, or **the workspace is not fingerprint-trusted**. A fingerprint-trusted
local keyboard session takes `trusted_local()`, whose `compute_secret_deny(Trusted, ..)` performs
**no workspace walk at all**. `with_project_secret_deny()` (the other caller of the walk) has **zero
production call sites** — only tests.

So the boot cost is paid on **first boot in an untrusted directory**, which is also the worst first
impression the product can make. That is worth fixing, but it is one walk, not two.

## MEASURED DECOMPOSITION (hetzner, debug build, controlled probe tree)

Probe tree ground truth (written to `/root/lane-boot-walk-groundtruth.txt`, read with the Read
tool, never `wc`): `DIRS_TOTAL=10013`, `DIRS_VIS=2001`, `DIRS_IGN=8001` (`node_modules/`, listed
in `.gitignore`), `FILES_TOTAL=30020`, plus a planted `node_modules/d1/hidden.env`.
Small probe: `SMALL_DIRS=162`, 150 of them ignored.

**There ARE two full no-prune traversals. Neither is repomap.** Proven at path level: the
directory `node_modules` itself is opened exactly **2** times, and `node_modules/dN` is opened
**16,000** times against a ground truth of 8,000 ignored dirs. A gitignore-respecting walker
(repomap) would open none of them.

Attribution, by PID + instrumentation in a SINGLE process:

| Walk | Thread | Evidence | On boot path? |
|------|--------|----------|---------------|
| `wcore_tools::workspace_policy::project_committed_secrets` | bootstrap thread | `Backtrace::force_capture()` says `project_committed_secrets` ← `compute_secret_deny` ← `WorkspacePolicy::contained` ← `bootstrap::build_scoped` ← **`wcore_cli::tui::splash_while`** ← `wayland_core::run`. Called **exactly once** (static counter), **294–367 ms** for 10k dirs / 30k files. | **YES — inside `splash_while`, so it blocks first paint** |
| `notify` recursive watcher (`wcore-agent/src/watch.rs:127`, `RecursiveMode::Recursive`, armed from `engine.rs:4888 install_file_watcher_eventually`) | detached `wcore-filewatcher-init` std::thread | second PID in the same trace does 300 dir opens AND **all 164** `inotify_add_watch` calls (ground truth 162 dirs). Zero `inotify_add_watch` on any other thread. | **NO — detached thread, does not block first paint, but contends for the same IO** |

Single-run correlation (`/root/lane-boot-walk-ino-result.txt`):
`pid 207654 → 150 node_modules opens, 0 inotify`; `pid 208193 → 300 node_modules opens, 164 inotify`;
`WLPROBE_ENTER count = 1`.

## Premises graded

| Brief claim | Verdict |
|---|---|
| "two full recursive walks of cwd" | **TRUE** (and I proved it at path level, which the brief did not) |
| "walk 1 = `wcore_repomap::scope::scope_files`" | **FALSE.** Not on the boot path, and a gitignore-respecting walker cannot produce the observed ignored-subtree opens. |
| "walk 2 = `project_committed_secrets`" | **TRUE**, and it is the one that actually blocks first paint. |
| walk 2's missing prune is deliberate and load-bearing | **TRUE** — comment at `workspace_policy.rs:828-836`, pinned by `workspace_policy/tests.rs:548-562`. |
| scoping agent: "3,649 `inotify_add_watch` comes from the recursive notify watch" | **TRUE**, and stronger than stated: that watcher is not just syscall noise, it is the *entire second traversal*. |
| scoping agent: the walk is posture-dependent (`Contained` only) | **TRUE in source** (`workspace_policy.rs:805`), and my instrumented run confirms the boot path reaches it via `WorkspacePolicy::contained`. |

## Instrument defects found (all repaired in-lane)
1. `wc -l` fabricated `0` for a 12-line file → all counts now read from files with the Read tool.
2. zsh ate an unquoted `--include=*.rs` → all globs quoted.
3. **`--trust-workspace --version` silently never grants** — `--version` returns before the grant
   at `main.rs:1811`. My first posture differential was therefore two *identical untrusted* runs
   reported as a trusted-vs-untrusted comparison, and it "showed no difference" for that reason.
   Caught by checking that `workspace-trust.json` existed; it did not. This is the
   "a participant never started" self-pass. Repaired: the grant is now asserted by the presence of
   the store file AND the `Trusted workspace executable fingerprint` line before any differential.
4. `strace -k` cannot reach the walk — 1,429 openat in 120 s (5.5 MB of stack text). Dead
   instrument for whole-boot attribution; replaced with in-process `Backtrace::force_capture()`
   plus PID correlation.

## Still to establish
- [ ] First-paint cost attributable to each walk at realistic scale.
- [ ] 4-way cross-audit of the remedy.
- [ ] Both-directions proof of whatever lands.
