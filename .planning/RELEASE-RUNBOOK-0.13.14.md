# 0.13.14 release runbook

Verified against the live repo 2026-09-10. Every fact here was read from the API
or the tree, not recalled.

## Preconditions, all checked

| fact | value | how |
|---|---|---|
| workspace version | `0.13.14` | `Cargo.toml` |
| npm `latest` and `next` | **0.13.12** | `npm view @ferroxlabs/wayland-core dist-tags` |
| `v0.13.14` tag | does not exist | `git/ref/tags/v0.13.14` -> 404 |
| `main` linear history | **required** | branch protection |
| `main` enforce_admins | **true** | branch protection |
| required reviews | none | branch protection |
| required checks | **13** | listed below |

**0.13.13 was never published to npm.** The public jump is 0.13.12 -> 0.13.14, and
the Desktop hand-off must say so rather than implying a 0.13.13 exists.

`enforce_admins: true` means the 13 checks are not bypassable. There is no
`--admin` path and none is to be attempted.

## The 13 required contexts on `main`

```
CI (linux-containerized)        Build (aarch64-apple-darwin)
CI (macos-latest)               Build (aarch64-pc-windows-msvc)
CI (Array)                      Build (aarch64-unknown-linux-gnu)
Eval acceptance gate            Build (x86_64-apple-darwin)
Bench regression (linux)        Build (x86_64-pc-windows-msvc)
report                          Build (x86_64-unknown-linux-gnu)
scan
```

**Three of these never run on `integ/**`** and only appear on the PR:
`Bench regression (linux)` (bench-regression.yml), `scan` (osv-scan.yml), and the
Supply Chain job — all three are `on: pull_request: branches: [main]`. Verified in
the workflow files. So a green integ run is NOT a prediction of a green PR, and
the PR must be opened early enough to surface them.

## Sequence

1. **Push** `integ/release-0.13.14` via the hetzner adapter (`just push` under
   `flock build.lock`, asserting HEAD). Read the `.status` receipt, NOT the ssh
   exit code — the ssh wrapper has reported 0 over a failed push.
2. **CI green** on the branch. Capture the run id: `release.yml` takes an
   `evidence_run_id` input described as "Successful current-SHA CI/collector run
   carrying `stabilization-raw-<SHA>`; **required for promotion**". Without it the
   promote step has no evidence to bind to.
3. **PR into `main`**, wait for all 13 including the three that only exist there.
4. **Squash-merge** (linear history is required). No `--admin`.
5. **Tag at the exact admitted source** — the merge commit on `main`, not the
   integ head, because a squash rewrites the SHA.
6. **Dispatch** `release.yml` with `--ref main` and `tag_name=v0.13.14`.
   **`--ref` stays `main`.** A tag ref breaks Azure OIDC — the subject claim is
   built from the ref and a tag ref produces a subject the federated credential
   does not match.
7. **Watch to `publish-npm`**, then confirm via the `release-complete` job, which
   fails on any non-success job, `draft != false`, asset `count < 14`, or a
   missing `release-manifest`.

## Deliver to Desktop

Published version; exact source SHA; authenticated release manifest; Windows x64
and Mac ARM64 binaries with SHA256; contract manifest. State absolute values per
binary, never "unchanged" — a relative statement is relative to a baseline the
reader may not share.

## Corrections measured 2026-09-10 (session C) — supersede the sections above

### The integ push already runs 11 of the 13 required checks
An earlier note in `HANDOFF-CLAUDE-0910-B.md` said three required checks never
run on `integ/**`. Measured against the last integ push run (34438211271), it is
**two**, and the eleven that DO run include every leg that was assumed late:

    Build (aarch64-apple-darwin)    Build (aarch64-pc-windows-msvc)
    Build (aarch64-unknown-linux-gnu)  Build (x86_64-apple-darwin)
    Build (x86_64-pc-windows-msvc)  Build (x86_64-unknown-linux-gnu)
    CI (Array)   CI (linux-containerized)   CI (macos-latest)
    Eval acceptance gate (Linux, containerized)   report

`ci.yml`'s `on:` block is `pull_request→main`, `push→main`, `push→'lane/**'`,
`push→'integ/**'`, and the darwin/windows matrix gates on
`startsWith(github.ref_name,'integ/')` — so an integ push gets the full matrix
WITHOUT needing the `[ci-darwin]` / `[ci-windows]` commit markers a `lane/**`
push needs. A green integ run therefore predicts the PR far better than assumed.

### The two that are genuinely PR-only, and what to do about each
- **`scan`** (`osv-scan.yml`) — also carries `workflow_dispatch`, so it can be
  fired against any ref before the PR exists:
  `gh workflow run osv-scan.yml -R FerroxLabs/wayland-core --ref <ref>`
- **`Bench regression (linux)`** (`bench-regression.yml`) — fires ONLY on
  `pull_request→main` / `push→main`, so it cannot be dispatched. It is the one
  check whose first execution is the PR. It does not need CI: the gate is
  `cargo run --bin wcore-eval-bench -- --floor 0.7 --report-json bench.json`
  over a 30-case corpus, exit 0 iff `pass_ratio >= 0.7`. Run that on hetzner
  before opening the PR and the last late surprise is gone. The 0.7 floor is
  pinned in BOTH the workflow and the binary default on purpose — if you change
  one, change both, deliberately.

### core#404 c1 does not have to wait for the PR
`ci.yml` fires on `push→'lane/**'`, so the cancellation demonstration can be
staged on a throwaway lane branch and never touches the release run. Sequence:
push the tree to `lane/<name>`, watch job `CI (linux-containerized)` until step
`Upload nextest JUnit checkpoint (survives a later cancellation)` (step 33 on
the last run) reaches `conclusion: success` AND a later step is `in_progress`,
then `gh run cancel`. Download `nextest-junit-linux-containerized-checkpoint`
and quote the report job's `CANCELLED WITH TEST EVIDENCE` line. Cancelling the
release run instead would cost a clean green and buy nothing.

### wayland#1272 c2 is one Sean action, not engineering work
Its whole residual was wayland#1256 c3. At this tree wayland#1256 reads c1 met,
c2 met, c3 **met** — `symbol:scripts/check-test-scope-coverage.py::receipts_for`
resolves at `scripts/check-test-scope-coverage.py:169` and the gate is wired at
`scripts/preflight.sh:416`. Five of tranche 3a's six are CLOSED on the tracker;
#1256 is the sixth and is release-closable on its merits. A lane may not close
an issue, so c2 stays not-met until `gh issue close FerroxLabs/wayland#1256`.

### The shared-home hazard is cross-PROCESS, not intra-process
Recorded because the earlier framing was wrong and would send the next lane to
the wrong instrument. `tools/remote-proof.py` exports one
`WAYLAND_HOME="$root/test-homes/$nonce/wayland-core"` for the WHOLE cargo
invocation, so every process it spawns resolves the same home. Nextest does not
hide this class — it sharpens it, because more processes means more contention.
Proven by pid mismatch (`F24_CHANNEL_LEASE=observer owner_pid=3931773` while the
panicking process was `3931789`) and by a two-process sqlite migration race that
one process cannot produce: both read `user_version` below target, both run the
same `ALTER TABLE`, the loser gets `duplicate column name: last_latency_ms`,
`Memory::open` returns `Err`, and bootstrap silently degrades to `NullMemory` —
which accepts every write and returns a fresh id, so one root cause surfaced as
several unrelated-looking assertion failures.

## The merge, step by step (written before it is needed, on purpose)

Six lanes are in flight. This is the order to land them in and the two traps
that have already cost this release time.

### Order
Land in dependency order, `check-criteria-ledger.py --offline` green before
EVERY commit, and `check-release-board.py --write` only at the very end:

1. `w15/lease` — isolation fixes + the wire-test remedy. **Drop its corpus
   commit `a43b64ac5`** (see below). Land first: it is what unblocks the push.
2. `w15/mac`, `w15/leak`, `w15/win-cred`, `w15/cron449`, `w15/skills401` — no
   known overlap, but run `git log --oneline HEAD..<lane>` and diff the touched
   paths against the already-landed set before each cherry-pick.
3. Re-anchor every `last_verified_commit` a cherry-pick orphaned. Cherry-picking
   orphans them EVERY time; the gate names each file, so let it.

### Trap 1 — `cargo fmt` FIRST, corpus SECOND. Never the other way.
`SOURCE_INPUTS` (`crates/wcore-protocol/src/contract/spec.rs:1330`) hashes files
BY PATH, and three of them are files a merge routinely reformats:

    crates/wcore-agent/src/bootstrap.rs   (spec.rs:1355)
    crates/wcore-agent/src/engine.rs      (spec.rs:1356)
    crates/wcore-cli/src/main.rs          (spec.rs:1361)

Regenerating the corpus and THEN running `cargo fmt` reformats a hashed input
and re-breaks the corpus you just fixed. So: `cargo fmt --all` (the one
Mac-safe cargo command), THEN regenerate.

### Trap 2 — regenerate ONCE, over the MERGED tree
Not once per lane. A lane that regenerates in its own worktree produces a digest
for a tree that will never ship, and its commit must be dropped on integration.
`w15/lease` already did this and its `a43b64ac5` is to be dropped.

    # on hetzner, in the merged worktree, under the slot flock
    cargo run -p wcore-protocol --bin wcore-contract -- generate
    cargo run -p wcore-protocol --bin wcore-contract -- diff
    cargo run -p wcore-protocol --bin wcore-contract -- digest

### The check that decides whether the regen was legitimate
`schema_digest` must be an **IDENTICAL SET** across the diff. Only
`fixture_digest` and `source_inputs_digest` may move. A moved `schema_digest`
means a WIRE SCHEMA changed — that is a Desktop-breaking change, not a
formatting artifact, and it must be understood before it is committed, never
blessed to make a gate green.

Expected value carried forward from the lease lane, which touched `bootstrap.rs`
(its only SOURCE_INPUTS file): `schema_digest` held at
`sha256:8497e92e…83fc6e33`. If the merged tree shows that value, the regen moved
only fixtures and inputs, as it should.

Then replay the checked corpus rather than trusting the generator:

    just desktop-contract-check

### Pushing the result
`just push` = `lint-fix fmt _auto-commit-fixes preflight test` and is FORBIDDEN
on the Mac. Run it on hetzner through the adapter: assert HEAD, take
`flock build.lock`, `vx just push`, write a `.status` receipt.
**READ THE `.status` RECEIPT, NEVER THE SSH EXIT CODE.** The ssh wrapper has
already reported 0 over a push whose receipt said `EXIT_CODE=1`.

## Both PR-only checks are now pre-verified (2026-09-10)

Neither has to wait for the PR any more, so no required check first executes
there:

- **`scan`** — dispatched via `gh workflow run osv-scan.yml --ref <ref>`,
  run 34474477558, **success**.
- **`Bench regression (linux)`** — reproduced off CI on hetzner at the merged
  tree `9d9fde501`:
  `WCORE_PROOF_SLOT=default python3 tools/remote-proof.py worktrees/release-0.13.14 run -p wcore-eval --bin wcore-eval-bench -- --floor 0.7`
  → `remote_exit 0`, `"complete": true`, **`bench: 30/30 passed (ratio 1.0000,
  floor 0.7000) -> OK`** (tool_routing 8/8, arithmetic 8/8, recall 8/8,
  file_ops 6/6).

CONFIRMED THE HARD WAY, and worth recording: CI run 34474108078 on a lane
branch cut at `5715b2b35` failed at step 26 `Clippy (warnings = errors)` — the
five `doc_overindented_list_items` in
`crates/wcore-cli/tests/quarantine_console_authority_windows.rs` from
`ad329925d`, a commit no CI run had ever linted. Fixed at `bbf1ecb09`. That red
would have failed the release push, and nothing but running the gate would have
found it.

### The ledger anchor test is weaker than it looks
`check-criteria-ledger.py` requires `last_verified_commit` to be an ANCESTOR of
HEAD. That does NOT mean it is at or after the work being graded: core#401's
anchor was left at `5715b2b35`, which passed the ancestor test while predating
the repair, so a reader re-deriving the grading would have built a tree without
it. Re-anchored to `6bc595704`. All 206 ledger anchors are ancestors of HEAD;
the pre-fix case is the one the gate cannot see.
