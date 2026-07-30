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

## Still to establish
- [ ] Re-measure the boot decomposition on hetzner with instrumentation, not strace attribution.
- [ ] Identify what the second walk actually is (if there is one).
- [ ] 4-way cross-audit of the remedy.
- [ ] Both-directions proof of whatever lands.
