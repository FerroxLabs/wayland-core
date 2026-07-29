# RECON — Grok Build (`xai-org/grok-build`)

Lane `recon-grok`, 2026-07-29. Peer read **read-only**; zero writes to
`/Users/seandonahoe/dev/resources/grok-build`. All load-bearing measurement via
`/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/find`, each with a known-positive control in the
same invocation.

**Why this peer matters:** it is the only same-language peer. Hermes is Python, OpenClaw is
TypeScript; both comparisons have been structural hand-waving. This one shares our language,
edition, ecosystem and most of our dependency graph — 610 crates in common. It has never
appeared in `COMPETITIVE-LEDGER.md`.

---

## 1. Pin block — ready to paste into `COMPETITIVE-LEDGER.md`

Matches the shape used for Hermes and OpenClaw (ledger lines 36-39 and 54-59).

### Baseline table row

| Peer | Repository | Baseline commit | Exact version | Version pin source | Commit date |
|---|---|---|---|---|---|
| Grok Build | `https://github.com/xai-org/grok-build.git` | `c68e39f60462f28d9be5e683d9cbe2c57b1a5027` | **0.2.102** | `git show origin/main:crates/codegen/xai-grok-version/Cargo.toml` → line 4 `version = "0.2.102"` (description: "Lockstepped grok CLI version.") | 2026-07-16 |

### HEAD table row

| Peer | HEAD commit | Version | Version pin source | `git describe --tags` | HEAD date |
|---|---|---|---|---|---|
| Grok Build | `98c3b2438aa922fbbe6178a5c0a4c48f85edc8ce` (`origin/main`) | 0.2.102 | as above | *(no tags in repo — `fatal: No names found`)* | 2026-07-17 |

### Pin caveats that must travel with the row

1. **The local checkout is NOT on upstream main.** Working tree HEAD is
   `a7d0968fe027b0e1f8e54c54d14e2ecba719a882`, branch `research/wayland-integration-audit`,
   tree **clean** (`git status --porcelain` → 0 lines). `a7d0968` is a **local** commit by
   `ci <sean@seandonahoe.com>` adding exactly one file — `WAYLAND-INTEGRATION-AUDIT.md`,
   +662 lines, **0 `.rs` touched** (`git show --name-status`; control: 1 `.md` matched).
2. Its parent `c68e39f` **is** an ancestor of `origin/main` (`merge-base --is-ancestor` → YES),
   so the baseline is legitimate — but `origin/main` is **2 commits ahead**, and the delta is
   304 files / +22,323 / −35,799. **The on-disk tree is materially stale.** Everything below is
   read from `origin/main` (`98c3b24`) via a scratchpad `git archive` extract.
3. **No archaeology is possible on this peer.** Entire history is 2 commits, both squashed
   monorepo dumps ("Publish harness and TUI open-source", "Synced from monorepo"). No PRs, no
   review trail, no per-feature commits. `SOURCE_REV` records the internal monorepo SHA
   `124d85bc5dc6e7805560215fcc6d5413944920e1`, which we cannot resolve.
4. `WAYLAND-INTEGRATION-AUDIT.md` in that tree is **ours, not xAI's** — 662 lines authored by
   `sean@seandonahoe.com`. Prior recon on this peer exists and never reached the ledger. Treat
   as prior-work reference, never as peer evidence.

---

## 2. Shape, and two corrections to our own premises

| | Grok Build | Wayland Core |
|---|---|---|
| Workspace members | **74** (62 `crates/codegen`, 11 `crates/common`, + build/prod) | **57** |
| Edition | 2024 | **2024** |
| Version | 0.2.102 | 0.12.25 |
| Lockfile packages | 1,125 | 900 |
| Shared packages | \multicolumn — **610 in common** | |

**Correction 1 — the brief's "our eleven `wcore-*` crates" is wrong, and so is `AGENTS.md`.**
We ship **57** workspace members (`sed -n '/^\[workspace\]/,/^\[/p' Cargo.toml | grep -cE '^\s*"'`).
`AGENTS.md`'s crate-map table lists ~19 and omits `wcore-acp`, `wcore-gateway`, `wcore-egress`,
`wcore-budget`, `wcore-swarm`, `wcore-permissions`, `wcore-dispatch`, `wcore-safety`,
`wcore-pricing`, `wcore-replay`, ten channel crates and more.

**Correction 2 — `AGENTS.md:214` says "Rust 2021 edition". We are on edition 2024**
(`Cargo.toml:156`, `[workspace.package]`). I nearly published "they are 2024, we are 2021" as a
delta on the strength of our own docs. It is not a delta. **Both stale-doc findings are ours to
fix and are the cheapest items in this report.**

So the decomposition gap is 74 vs 57 — comparable, not 4x. **We are not structurally behind on
modularity.** The gaps are in *what the crates do*, not how many there are.

**Non-finding, recorded because I chased it:** I initially concluded the product binary was
withheld (only 2 `src/main.rs` in the tree, none named `grok`). Wrong — the README layout table
names `xai-grok-pager-bin` as the **composition root**, building binary `xai-grok-pager`; the
`grok` name is applied at packaging. The export is complete and buildable.

---

## 3. Crate-by-crate comparison

Only rows where there is a real signal. LOC = `find src -name '*.rs' | xargs cat | wc -l`.

| Their crate | LOC | Purpose | Our nearest | Ahead | Why |
|---|---|---|---|---|---|
| `xai-acp-lib` | 2,277 | ACP over the **published** `agent-client-protocol 0.10.4` crate | `wcore-acp` (9,899) | **Them** | They consume the spec crate; we hand-maintain a 4x-larger reimplementation whose own header says integration is unfinished. See §4.2 |
| `xai-codebase-graph` | 8,909 | tree-sitter query-based code graph | `wcore-repomap` | **Them** | AST-grade vs our "aider-style light symbol extractor". `tree-sitter` in our tree: **0 files** |
| `xai-sqlite-journal` | 779 | Network-FS-aware journal-mode selection | *(none)* | **Them** | We have a live defect here. See §4.1 |
| `xai-system-power` | 743 | Cross-platform suspend/wake notifications | *(none)* | **Them** | See §4.4 |
| `xai-hunk-tracker` | 13,012 | Diff hunks with **agent vs external attribution** | *(none)* | **Them** | See §4.5 |
| `xai-fast-worktree` | 18,170 | CoW-cloned git worktrees (`reflink-copy`) | *(none)* | **Them** | Largest crate in their tree. We shell out to `git worktree` |
| `xai-grok-sandbox` | 3,729 | Landlock/Seatbelt via `nono` | `wcore-sandbox` | **Mixed** | They cover Linux+macOS via one crate; **we also cover Windows AppContainer**, which they do not. Ours is broader, theirs is less code |
| `xai-computer-hub-sdk` | 14,254 | Remote tool hub: connection pool, transparent reconnect, tool-server runtime | `wcore-cua` + `wcore-browser` + remote-reach | **Them, on durability** | Transparent reconnect + pooling is exactly the durability layer our remote surfaces lack |
| `xai-grok-hooks` | 7,613 | File-discovered hooks + policy enforcement | `wcore-agent/src/hooks` | ~Even | Comparable |
| `xai-grok-memory` | 9,918 | Memory | `wcore-memory` | ~Even | Both substantial; not differentiated by reading alone |
| `xai-grok-update` | 6,061 | Self-update | *(installer scripts)* | **Them** | 6k LOC of in-process update vs our shell installers |
| `xai-grok-plugin-marketplace` | 5,502 | Plugin marketplace client | `wcore-plugin-*` (3 crates) | **Mixed** | They have distribution; we have **wasm + subprocess isolation + SHA-256-bound approval gates** they do not |
| `xai-grok-compaction` | 6,790 | Transport-agnostic compaction, **shared with Grok chat** | `wcore-compact` | ~Even | Note the reuse across two products |
| `xai-interjection-core` | 320 | Mid-turn interjection buffer | `wcore-agent` prompt paths | **Them** | Tiny, sharp, factored out for client+server reuse |
| `xai-circuit-breaker` | 2,176 | Circuit breaker | scattered retry | **Them** | Ours is not a named, testable unit |
| `xai-grok-voice` | 2,802 | Streaming STT dictation | `voice_mode.rs` | **Them** | Dedicated crate + `voice-probe` bin vs one file |
| `xai-grok-secrets` | 568 | Outbound scrubber for Sentry/Mixpanel | `wcore-egress` | **Us** | Ours is a policy boundary; theirs is a regex scrubber on telemetry only |
| *(none)* | — | — | `wcore-channel-*` ×10, `wcore-gateway` | **Us** | **They have no channels/gateway plane at all.** See §6 |

---

## 4. The five things they do better

### 4.1 SQLite WAL is unsafe on network filesystems — they fixed it, we have the bug

`crates/codegen/xai-sqlite-journal/src/lib.rs:3-12` documents the failure precisely: WAL keeps
its wal-index in an mmap'd `-shm` file requiring coherent shared memory and reliable POSIX
locks, which network filesystems do not provide. With an NFS-mounted `$HOME` on several
machines, a peer host rebuilding `-shm` during WAL recovery "rips the backing out from under
our mapping and the next wal-index read dies with **SIGBUS**". Their fix: detect network FS
(`is_network_fs(dir)`, line 66), fall back to `truncate` rollback journal, **use a per-host DB
filename** so an old binary cannot flip a shared DB back to WAL (lines 90-96), plus a
`GROK_SQLITE_JOURNAL_MODE` field kill-switch (line 38).

**We set `PRAGMA journal_mode = WAL` unconditionally in four places:**
`wcore-repomap/src/store.rs:211`, `wcore-memory/src/db.rs:394`,
`wcore-memory/src/schema/mod.rs:45`, `wcore-swarm/src/audit.rs:196`.

**Absence claim, with its query** (§3b-i discipline). Query:
`/usr/bin/grep -rn -iE 'nfs|cifs|smbfs|statfs|f_type|network.?f(ile)?s' crates --include='*.rs'`.
Result: **no network-FS detection anywhere**. Every hit was a false positive — the substring
`infs` in a local variable (`wcore-honcho-adapter`, `wayland-honcho` tests) and one unrelated
comment about path depth at `wcore-skills/src/paths.rs:92`. Instrument proven alive on the same
files: `grep -rl 'rusqlite' crates --include='*.rs'` → **33 files**.

This is the single most actionable finding in the report: small, self-contained, and it is a
real corruption/SIGBUS class on NFS `$HOME`, which is normal in enterprise and CI.

### 4.2 They consume protocol standards; we reimplement them

- **ACP:** they depend on `agent-client-protocol 0.10.4` (+ `agent-client-protocol-schema`) from
  crates.io, used by **10 of their crates**. `xai-acp-lib` is a 2,277-line adapter.
  We ship `wcore-acp` at **9,899 lines** with our own `protocol.rs` types and **zero**
  dependency on the published crate (query:
  `grep -rn 'agent-client-protocol' --include='*.toml' --include='*.rs'` → only two doc
  comments, no manifest entry; control: `grep -rc 'serde' Cargo.toml` → 8).
  Our `wcore-acp/src/lib.rs:8` says: *"Client 1.A.7 + engine integration 1.A.10 still to land."*
  Also: both doc comments cite `github.com/anthropics/agent-client-protocol`; the crate they
  actually use is the Zed-originated one. Worth checking we implemented the spec we think.
- **MCP:** they use `rmcp` + `rmcp-macros` (the official Rust MCP SDK). We have **no `rmcp`**
  (`grep -cx 'rmcp' <our lock>` → 0) and hand-roll `wcore-mcp`.

Two spec surfaces, both hand-maintained by us, both with maintained upstream crates available.
That is recurring cost we are paying for nothing.

### 4.3 tree-sitter code intelligence, and an LSP client

`xai-codebase-graph` (8,909 LOC, 26 files, ships a `code-graph` binary + two benches) builds a
code graph from **tree-sitter queries**. They additionally carry `async-lsp` and `lsp-types` —
they can drive real language servers.

Our side: `tree-sitter` → **0 files**. LSP → **0 files**, on a word-boundary query
(`grep -rliE '\blsp\b|language server|textDocument/'`) with the instrument proven alive at
**144** files for `\bmcp\b`. My first LSP sweep returned 81 files and was pure substring noise;
the refined query is the honest one.

`wcore-repomap` is self-described as a "light symbol extractor". Against an AST-grade graph plus
LSP, our code-understanding layer is the weakest comparison in this report.

### 4.4 They handle the machine going to sleep

`xai-system-power` (743 LOC): "Cross-platform system sleep/wake (suspend) notifications — used
to **defer work across a suspend boundary**", exposing `PowerEvent`, `PowerState`,
`current_power_state()`, `SystemPowerListener::start(callback)`.

We have a durable task ledger, cron, schedule leases and a gateway with delivery
idempotency — all of which assume the host stays awake. A laptop suspending mid-delivery is a
first-class failure mode for exactly the plane Phase 24 just built. **Nothing in our tree
listens for suspend.** 743 lines is a cheap hardening of work we have already paid for.

### 4.5 Edit provenance — agent versus human

`xai-hunk-tracker` (13,012 LOC): "Track file hunks (diffs) with **agent/external attribution**".
The agent knows which hunks it wrote and which a human (or another tool) wrote underneath it.

We have no counterpart. This is the substrate for safe re-edit, conflict detection, honest
"what did the agent actually change" reporting, and undo — and it is directly relevant to our
delegated-mutation and checkpoint work.

---

## 5. Dependency deltas worth acting on

610 shared packages, so these are genuine choices, not ecosystem drift.

| Area | They use | We use | Read |
|---|---|---|---|
| MCP | `rmcp`, `rmcp-macros` | hand-rolled `wcore-mcp` | **Adopt.** Official SDK |
| ACP | `agent-client-protocol` 0.10.4 | 9,899-line `wcore-acp` | **Evaluate.** Big maintenance delta |
| Git | `gix` (~60 crates) + `git2` + `libgit2-sys` | **neither** — we shell out to `git` | **Strong.** In-process git kills a whole class of shell-injection and parsing bugs |
| Process control | `process-wrap`, `command-fds`, `os_pipe` | hand-rolled | We landed `fix(sandbox): own and reap process trees` (`2b662fe8`) days ago. `process-wrap` is that problem, solved upstream |
| Code intel | `tree-sitter`, `async-lsp`, `lsp-types` | none | **Strong.** See §4.3 |
| Search ranking | `bm25` | none | Our index (23B-03) has no lexical ranker |
| Sandbox | `nono` (Landlock+Seatbelt unified) | hand-rolled per-OS | Ours also covers Windows; theirs is less code for Linux+macOS |
| **Media/multimodal** | `resvg`, `pdf_oxide`, `jpeg-decoder`, `fast_image_resize`, `imagesize`, `infer`, `hayro-{ccitt,jbig2}`, `qcms`, `fontdb`, `read-fonts` | `image` only | **Widest capability gap.** They render SVG, PDF, fax-encoded images and fonts in-process. This is our named weak area |
| Diagrams | `mermaid-to-svg`, `dagre_rust`, `graphlib_rust` + vendored `third_party/` Mermaid | none | Terminal-rendered Mermaid |
| Terminal | `alacritty_terminal`, `ansi-to-tui`, `arboard`, `cursor-icon`, `dark-light` | `ratatui` alone | Real VT emulation inside the TUI |
| Tracing | `fastrace` + `fastrace-opentelemetry`/`-reqwest`/`-tonic` | `opentelemetry` + tracing | `fastrace` is materially faster; we own `wcore-observability` |
| Caching | `moka`, `clru`, `cached` | none of these | Cache economics (F23-04) is unbuilt on our side |
| Testing | `insta` (snapshots), `proptest`, `pretty_assertions` | neither `insta` nor `proptest` | **Cheap win.** Property + snapshot testing for protocol/compaction |
| Perf/metrics | `pprof`, `hdrhistogram`, `prometheus`, `mimalloc` | none | They profile and export metrics as a matter of course |
| Time | `jiff` | `chrono` | `jiff` is the modern correct-by-default choice |
| Schema | `jsonschema` | none | Tool-schema validation |
| Cloud | `aws-sdk-s3`, `gcloud-storage`, `oauth2` | `bollard`, cloud SDKs differ | They have real object storage + OAuth |

**Ours they lack** (our genuine surface area): `candle-{core,nn,transformers}` (local
inference), `chromiumoxide` (browser), `cap-std`/`cap-*` (capability-based FS — a real security
architecture they have no counterpart to), `bollard` (Docker), `cranelift`/wasm (plugin
sandboxing), `dbus-secret-service`, `argon2`/`chacha20poly1305`/`aead` (real crypto),
`async-tungstenite`, `cron`, `calamine`.

---

## 6. Things they do that neither we, OpenClaw nor Hermes do

1. **Suspend/wake awareness** (`xai-system-power`) — §4.4. I know of no counterpart in either
   other peer.
2. **Network-FS-aware SQLite journal selection with per-host DB naming** (`xai-sqlite-journal`)
   — §4.1. Extremely specific production-earned knowledge.
3. **CoW git worktree cloning** (`xai-fast-worktree`, 18,170 LOC, `reflink-copy`) — filesystem
   reflinks to make agent worktrees near-free.
4. **Mermaid rendered in the terminal** — a vendored diagram stack (`third_party/`) plus
   `dagre_rust`/`graphlib_rust` layout engines.
5. **A compaction engine shared across two products** (`xai-grok-compaction`, "Grok chat **and**
   Grok Build") — they factored context compaction as a product-independent library.

**Where we are ahead of them, plainly:** they have **no channels plane and no gateway/service
plane at all** — no counterpart to our ten `wcore-channel-*` crates, `wcore-gateway`,
`wcore-channels-registry`, or pairing/delivery/drain. That is the widest gap in *our* favour and
it is the axis on which Hermes and OpenClaw beat us. Also ours alone: capability-based FS
(`cap-std`), wasm plugin isolation, local inference (`candle`), Windows AppContainer sandboxing,
and real crypto primitives.

---

## 7. Honest read for Sean

**Their architecture beats ours in one place that matters strategically and several that matter
tactically.**

Strategically: **code understanding.** tree-sitter graph + LSP + BM25 ranking against our
regex-grade `wcore-repomap` with no LSP and no ranker. For a *coding* agent that is the core
competency, and it is the one gap where I would point a build wave.

Tactically, in descending value-per-line: the SQLite network-FS fix (we have the live bug, ~779
lines to port the idea), suspend/wake (743 lines, hardens the durable plane we just built),
`rmcp` and `agent-client-protocol` adoption (deletes ~10k lines of our maintenance), and hunk
attribution.

**They do not beat us on breadth.** 74 crates vs 57 is not a real gap, and our channels/gateway
plane, capability-based FS, wasm plugin isolation and Windows sandboxing have no counterpart in
their tree. Their strength is *depth on the single-developer terminal coding loop*; ours is
*breadth across delivery surfaces and isolation*. They are a better coding agent; we are a
broader agent platform.

**The most uncomfortable finding is about us, not them:** two of my starting premises came from
our own `AGENTS.md` and both were false (edition 2021, eleven crates). Our documented crate map
omits ~38 crates. We have been comparing ourselves to peers using a description of ourselves
that is two editions and three dozen crates out of date.

---

## 8. What I did NOT do

- **No dead-surface sweep of their tree.** The brief asked for "impressive but dead" and I have
  no verified instance. I checked the one candidate I found (missing product binary) and
  **disproved it** — `xai-grok-pager-bin` is the composition root per the README layout table.
  Reporting no finding rather than a guess.
- **Did not compile, run or install anything** from the peer. Read-only throughout.
- **Did not read `WAYLAND-INTEGRATION-AUDIT.md`'s 662 lines** beyond establishing provenance. It
  is our own prior recon; it should be reconciled into the ledger, but it is not peer evidence
  and I did not want its conclusions contaminating a fresh read.
- **Did not verify their capability claims behaviourally** — no binary was run. Every "they
  have X" here is a source/manifest reading, exactly the standard `PEER-PROBE` uses: presence of
  a counterpart, never a performance or effectiveness claim.
- **Did not measure `xai-grok-memory` vs `wcore-memory` in depth**; I marked them "~even" on LOC
  and crate structure alone, which is weak evidence. Left explicitly unresolved.
