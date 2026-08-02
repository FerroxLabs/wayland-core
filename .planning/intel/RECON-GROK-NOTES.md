# RECON-GROK — working notes (append-only, committed continuously)

Lane `recon-grok`. Peer: `/Users/seandonahoe/dev/resources/grok-build`. READ-ONLY on peer.
**Final report: `RECON-GROK.md`. These are the raw measurements behind it.**

## Instrument discipline

- All load-bearing measurement via `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/find`.
- Peer read from a **scratchpad extract of `origin/main`** (`git archive origin/main | tar -x`),
  NOT the peer working tree. Zero writes to the peer.

## MEASURED — pin (t+8min)

- remote `https://github.com/xai-org/grok-build.git`; working tree HEAD
  `a7d0968f…`, branch `research/wayland-integration-audit`, tree CLEAN (0 porcelain lines).
- `a7d0968` is LOCAL, by `ci <REDACTED-MAINTAINER@redacted.invalid>`, adds ONLY
  `WAYLAND-INTEGRATION-AUDIT.md` +662, **0 `.rs`** (control: 1 `.md` matched).
- parent `c68e39f` IS an ancestor of `origin/main` (`merge-base --is-ancestor` → YES).
- `origin/main` = `98c3b24` (2026-07-17), **2 commits ahead**; delta 304 files
  +22,323/−35,799. On-disk tree materially stale.
- version **0.2.102** from `crates/codegen/xai-grok-version/Cargo.toml:4`.
- `SOURCE_REV` = `124d85bc5dc6e7805560215fcc6d5413944920e1` (internal monorepo, unresolvable).
- no tags (`git describe --tags` → fatal: No names found).

### Correction 1 (caught before publishing)
`git diff --stat origin/main..HEAD` looked like "someone deleted 35k lines from the peer".
Wrong — direction is upstream-relative; local commit is docs-only.

## MEASURED — shape (t+12min)

- peer 74 workspace members (62 codegen, 11 common); 2 commits total history, both squashed
  monorepo dumps. No archaeology possible.
- peer edition 2024.

### Correction 2 — TWO of our own premises were false
- **We are edition 2024**, `Cargo.toml:156` `[workspace.package]`. `AGENTS.md:214` claims
  "Rust 2021 edition". STALE DOC. I nearly published a false delta from our own docs.
- **We have 57 workspace members**, not "eleven wcore-* crates". `AGENTS.md` crate map lists
  ~19 and omits ~38 incl. `wcore-acp`, `wcore-gateway`, `wcore-egress`, `wcore-budget`,
  `wcore-swarm`, `wcore-permissions`, ten channel crates.

### Correction 3 — "product binary withheld" DISPROVED
Only 2 `src/main.rs` and none named `grok` → I suspected a partial export. README layout table
names `xai-grok-pager-bin` the **composition root** (builds `xai-grok-pager`); `grok` is a
packaging rename. Export is complete. Recorded as a non-finding.

## MEASURED — deps (Cargo.lock name extraction, both sides)

peer 1,125 packages, ours 900, **610 shared**.
- peer-only, high signal: `rmcp`+`rmcp-macros`, `agent-client-protocol`(+schema),
  `tree-sitter`, `async-lsp`+`lsp-types`, `bm25`, `gix`(~60)+`git2`, `nono`, `process-wrap`,
  `resvg`/`pdf_oxide`/`jpeg-decoder`/`fast_image_resize`/`imagesize`/`infer`/`hayro-*`,
  `mermaid-to-svg`/`dagre_rust`/`graphlib_rust`, `alacritty_terminal`, `fastrace*`,
  `moka`/`clru`/`cached`, `insta`, `proptest`, `pprof`/`hdrhistogram`/`prometheus`,
  `mimalloc`, `jiff`, `jsonschema`, `aws-sdk-s3`/`gcloud-storage`/`oauth2`, `sentry`.
- **we have NEITHER `gix` NOR `git2`** (both `grep -cx` → 0) — we shell out to `git`.
- ours-only: `candle-*`, `chromiumoxide`, `cap-std`/`cap-*`, `bollard`, `cranelift`/wasm,
  `dbus-secret-service`, `argon2`/`chacha20poly1305`/`aead`, `async-tungstenite`, `cron`.

## MEASURED — absence claims, each with query + live control

- **tree-sitter, our tree:** `grep -rliE 'tree.?sitter' crates --include='*.rs'` → **0**.
- **LSP, our tree:** first sweep `-liE 'language.?server|lsp'` → 81 files = pure substring
  noise. Refined word-boundary `'\blsp\b|language server|textDocument/'` → **0**;
  control `'\bmcp\b'` → **144 files**. The refined query is the honest one.
- **network-FS awareness, our tree:**
  `grep -rn -iE 'nfs|cifs|smbfs|statfs|f_type|network.?f(ile)?s' crates --include='*.rs'`
  → **no detection**; all hits false positives (`infs` local var; one unrelated comment at
  `wcore-skills/src/paths.rs:92`). Control `grep -rl 'rusqlite'` → **33 files**.
- **our unconditional WAL:** `wcore-repomap/src/store.rs:211`, `wcore-memory/src/db.rs:394`,
  `wcore-memory/src/schema/mod.rs:45`, `wcore-swarm/src/audit.rs:196`.
- **`agent-client-protocol` in our manifests:** none (only 2 doc-comment mentions);
  control `grep -rc 'serde' Cargo.toml` → 8. `rmcp` in our lock → 0.

## Peer crate sizes (LOC via find+cat+wc)

fast-worktree 18,170 · computer-hub-sdk 14,254 · hunk-tracker 13,012 · memory 9,918 ·
codebase-graph 8,909 · hooks 7,613 · compaction 6,790 · update 6,061 · marketplace 5,502 ·
tool-protocol 4,383 · sandbox 3,729 · tool-runtime 3,226 · voice 2,802 · acp-lib 2,277 ·
circuit-breaker 2,176 · computer-hub-core 1,421 · mcp-adapter 1,039 · sqlite-journal 779 ·
system-power 743 · secrets 568 · interjection-core 320 · token-estimation 255 ·
prompt-queue 176.

## Key evidence lines

- `xai-sqlite-journal/src/lib.rs:3-12` WAL/`-shm`/SIGBUS on NFS; `:66` `is_network_fs(dir)`;
  `:90-96` per-host DB name; `:38` `GROK_SQLITE_JOURNAL_MODE` kill-switch.
- `xai-system-power/src/lib.rs:38,74,95,133,149` PowerEvent/PowerState/listener.
- `wcore-acp/src/lib.rs:8` — "Client 1.A.7 + engine integration 1.A.10 still to land."
- `wcore-acp` cites `github.com/anthropics/agent-client-protocol`; the real crate is
  Zed-originated. Worth checking which spec we implemented.

## Provenance

`WAYLAND-INTEGRATION-AUDIT.md` (662 lines) in the peer tree is **ours**
(REDACTED-MAINTAINER@redacted.invalid), not xAI's. Prior recon exists and never reached
`COMPETITIVE-LEDGER.md`. Read for provenance only; not used as peer evidence.
