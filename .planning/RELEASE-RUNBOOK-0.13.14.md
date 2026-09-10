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
