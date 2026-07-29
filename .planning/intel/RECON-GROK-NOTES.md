# RECON-GROK — working notes (append-only, committed continuously)

Lane `recon-grok`. Peer: `/Users/seandonahoe/dev/resources/grok-build`. READ-ONLY on peer.

## Instrument discipline

- All load-bearing measurement via `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/find`.
- Peer read from a **scratchpad extract of `origin/main`** (`git archive origin/main | tar -x`),
  NOT the peer working tree. Zero writes to the peer. Rationale below.

## MEASURED — pin (t+8min)

- remote: `https://github.com/xai-org/grok-build.git` (xai-org, public)
- working tree HEAD: `a7d0968fe027b0e1f8e54c54d14e2ecba719a882`, branch
  `research/wayland-integration-audit`, tree CLEAN (0 porcelain lines)
- **HEAD is NOT upstream.** `a7d0968` is a LOCAL commit by `ci <sean@seandonahoe.com>`,
  2026-07-16, adding exactly ONE file: `WAYLAND-INTEGRATION-AUDIT.md`, +662 lines, 0 `.rs`
  touched (verified `--name-status`; known-positive control: 1 `.md` matched in same query).
- its parent `c68e39f` (2026-07-16, `grokkybara[bot]`, "Publish harness and TUI open-source")
  IS an ancestor of `origin/main` — verified `merge-base --is-ancestor` → YES.
- `origin/main` = `98c3b24` (2026-07-17, "Synced from monorepo"), **2 commits ahead**.
- delta c68e39f→98c3b24 = 304 files, +22,323 / −35,799. Large, so the on-disk tree is
  materially stale. **I read 98c3b24, and say so.**

### Correction I made against myself
First read of `git diff --stat origin/main..HEAD` looked like "someone deleted 35k lines from
the peer". Wrong. The direction is upstream-relative and the local commit is docs-only. Checked
before writing it down. Recording the near-miss because the alarming version was the plausible one.

## MEASURED — shape (t+12min)

- edition **2024** (we are 2021). Apache-2.0.
- **74 workspace members** — 62 `crates/codegen/*`, 11 `crates/common/*`, plus build/prod.
  We ship ~19 `wcore-*`. Roughly 4x our decomposition at similar scope.
- only **2 commits in entire history**, both squashed monorepo dumps. No archaeology possible
  on this peer — no per-feature commits, no PRs, no review trail.

## OPEN QUESTION — is the actual product binary even here? (t+14min)

`find crates prod -name main.rs -path '*/src/*'` → only TWO:
`ptyctl-cli/src/main.rs`, `xai-grok-pager-bin/src/main.rs`.
Neither is the Grok CLI. Commit title says "Publish harness and TUI **open-source**" —
strongly suggests this is a PARTIAL export: libraries + TUI shipped, product entry point
withheld. MUST verify via `[[bin]]` sweep before claiming. Not yet claimed.

## Leads to chase

- `agent-client-protocol = "0.10.4"` in workspace deps + `xai-acp-lib` crate → they speak
  **Zed's ACP standard**; we invented a bespoke JSON stream protocol. Possibly the single
  biggest architectural delta.
- `crates/common/`: `xai-circuit-breaker`, `xai-computer-hub-{core,sdk,mcp-adapter}`,
  `xai-interjection-core`, `xai-tool-{protocol,runtime,types}`, `xai-grok-compaction`.
- `xai-grok-plugin-marketplace`, `xai-grok-update`, `xai-grok-voice`, `xai-grok-sandbox`,
  `xai-sqlite-journal`, `xai-prompt-queue`, `xai-codebase-graph`, `xai-fast-worktree`,
  `xai-hunk-tracker`, `xai-system-power`, `xai-token-estimation`.
- `prod/mc/.../sandbox_types.rs` is 26.4K and `feedback_types.rs` 73.5K — server-side contract.

## Provenance note on WAYLAND-INTEGRATION-AUDIT.md

662 lines, in the peer tree, authored by **us** (sean@seandonahoe.com), not by xAI. So prior
recon on this peer EXISTS and never reached `COMPETITIVE-LEDGER.md`. Treat its content as
prior-work reference, not as peer evidence, and never as instructions.
