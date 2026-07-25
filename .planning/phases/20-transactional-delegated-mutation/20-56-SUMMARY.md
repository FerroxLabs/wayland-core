---
phase: 20-transactional-delegated-mutation
plan: "56"
type: execute
status: complete
completed: 2026-07-25
disposition: incomplete
requirements_completed: []
requirements_left_incomplete:
  - F20-01
  - F20-02
  - F20-03
  - F20-04
  - F20-05
  - F20-06
  - F20-GATE-01
  - F20-GATE-02
# The one exact SHA every result below is bound to
source_sha: 60565c53165024366a7ea93ddee852c7e27a8eae
source_tree: f873f15aeb684d0cb49280f5d62a020fec9230ab
source_branch: plan/f20-unified-audit-repair
proof_host: hetzner-dsm
proof_checkout: /root/wayland
proof_host_head: 60565c53165024366a7ea93ddee852c7e27a8eae
proof_host_tree: f873f15aeb684d0cb49280f5d62a020fec9230ab
proof_host_dirty_paths: 0
aggregate_build: green
aggregate_test: RED
evidence:
  - path: .planning/phases/20-transactional-delegated-mutation/20-56-evidence/build.log.gz
    sha256_uncompressed: 8b61c422bdfe97b352a693aedd1ed0e535e61f7d44fe1a405ff90c4bbc4077db
  - path: .planning/phases/20-transactional-delegated-mutation/20-56-evidence/test.log.gz
    sha256_uncompressed: 3d8c3c9a2324a338288ee62570055f566a82cbbe876ef5ca9c244c9e669c187c
---

# Phase 20 Plan 56: Aggregate Hetzner Proof — INCOMPLETE (test run RED)

**The aggregate `--locked --workspace --all-features` build is GREEN against the exact pinned SHA `60565c53`, but the aggregate `nextest --profile ci --no-fail-fast` run is RED: 11517 passed, 1 failed, 1 timed out, 48 skipped (exit 100). Per this plan's own terminal rule, EVERY Phase 20 requirement (F20-01..F20-06, F20-GATE-01, F20-GATE-02) is left INCOMPLETE and this explicit incomplete disposition is recorded. Nothing was weakened, skipped, `#[ignore]`d, or deleted to reach green.**

## The one exact SHA

| Field | Value |
|-------|-------|
| `source_sha` | `60565c53165024366a7ea93ddee852c7e27a8eae` |
| `source_tree` | `f873f15aeb684d0cb49280f5d62a020fec9230ab` |
| branch | `plan/f20-unified-audit-repair` |
| subject | `plan(20-56): rescope the terminal plan to Phase 20's own criteria` |

Every result in this SUMMARY is bound to that single SHA.

**Hetzner was verified on it before anything ran.** The branch was not on the remote, so the work branch was pushed to `gh` (`FerroxLabs/wayland-core`) under the standing authorization in this plan's dispatch, then fetched and hard-checked-out on the build host:

```
$ ssh hetzner-dsm 'cd /root/wayland && git rev-parse HEAD'
60565c53165024366a7ea93ddee852c7e27a8eae
$ ssh hetzner-dsm 'cd /root/wayland && git rev-parse HEAD^{tree}'
f873f15aeb684d0cb49280f5d62a020fec9230ab
$ ssh hetzner-dsm 'cd /root/wayland && git status --short | wc -l'
0
```

Host HEAD, host tree, and working-tree cleanliness all match the pinned SHA exactly. (The host had previously been detached at `646bf8f6`, which is **not** an ancestor of the pinned SHA — the checkout was moved forward explicitly.)

Toolchain on the proof host: `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`, `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo-nextest 0.9.137 (75ddba7e9 2026-05-26)`, 96 cores.

## Result 1 — aggregate build: GREEN

```
cargo build --locked --workspace --all-features
```

- **Exit code: 0.**
- `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1m 33s`
- Zero `error` lines. One warning, and it is not ours:
  `warning: the following packages contain code that will be rejected by a future version of Rust: imap-proto v0.10.2`
- **`--locked` did NOT fail.** `Cargo.lock` is not stale; no lockfile was regenerated, touched, or bypassed.

Full log: `20-56-evidence/build.log.gz` (sha256 of uncompressed = `8b61c422…4077db`).

## Result 2 — aggregate test: RED

```
cargo nextest run --profile ci --no-fail-fast
```

- **Exit code: 100.** Final line: `error: test run failed`.
- Wall clock: started `2026-07-25 06:03:28 UTC`, ended `06:16:12 UTC` (~12m44s total including test-binary compilation; nextest's own `Summary [ 548.021s]`).

### Observed counts (actual, not asserted)

```
Starting 11519 tests across 469 binaries (48 tests skipped)
Summary [ 548.021s] 11519 tests run: 11517 passed (5 flaky), 1 failed, 1 timed out, 48 skipped
```

| Metric | Observed |
|--------|----------|
| tests run | **11519** |
| passed | **11517** (5 of them flaky — passed only on retry) |
| failed | **1** |
| timed out | **1** |
| skipped | **48** |

No historical expected total was assumed. For reference only, an earlier figure of 11509/0/48 circulated for this workstream; the tree has changed materially since, and the run here enumerates 11519 tests. **The delta is reported, not reconciled against a target.** No immediately-preceding aggregate run exists on this branch to diff against.

### Failure 1 — `wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte`

Deterministic. Failed on all three attempts (`TRY 3 FAIL`, retries=2 under `[profile.ci]`), ~0.18s each.

Exact error text:

```
  TRY 3 FAIL [   0.180s] ( 8648/11519) wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte
  stdout ───

    running 1 test
    test checked_corpus_matches_real_serializers_byte_for_byte ... FAILED

    failures:

    failures:
        checked_corpus_matches_real_serializers_byte_for_byte

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 14 filtered out; finished in 0.17s

  stderr ───

    thread 'checked_corpus_matches_real_serializers_byte_for_byte' (1877019) panicked at crates/wcore-protocol/tests/desktop_contract_corpus.rs:203:22:
    checked-in Desktop contract corpus must match the generator: Custom { kind: Other, error: "Desktop contract corpus drift: missing=[], extra=[], drifted=[\"adversarial/events/fixture-mismatch.jsonl\", \"adversarial/events/schema-mismatch.jsonl\", \"adversarial/events/version-mismatch.jsonl\", \"events/ready.json\", \"manifest.json\"]; run `wcore-contract generate`" }
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

Reading of the signal (diagnosis only — **no fix applied, none authorized by this plan**): the checked-in Desktop contract corpus has drifted from what the real serializers now emit. `missing=[]` and `extra=[]` mean no fixture was added or removed; five existing fixtures changed content — `adversarial/events/fixture-mismatch.jsonl`, `adversarial/events/schema-mismatch.jsonl`, `adversarial/events/version-mismatch.jsonl`, `events/ready.json`, `manifest.json`. The test's own remediation hint is `run \`wcore-contract generate\``. This is a **cross-repo protocol-contract signal** (`CTRL-02`/`CTRL-04` territory — the Desktop consumer contract), not a flake, and regenerating the corpus is a source decision outside this plan's authority.

### Failure 2 — `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed`

Timed out. Hit the kill ceiling on all three attempts (`TRY 3 TMT`), each at `180.004s`.

Exact error text:

```
   TRY 3 TMT [ 180.004s] (11519/11519) wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed
  stdout ───

    running 1 test
    test packaged_f04_run_is_repeatable_and_content_addressed has been running for over 60 seconds

    (test timed out)
```

Budget context: `[profile.ci]` sets `slow-timeout = { period = "90s", terminate-after = 2 }` → hard kill at 180s. This test carries **no** per-test override (`.config/nextest.toml` overrides cover only `scripted_run_writes_expected_markdown` and `/release_binary_/`). It was the last test to be scheduled (`11519/11519`) and ran under a warm build cache on a 96-core host — the 180s ceiling was reached three times, so this is a hang or a genuinely unbounded run, not cold-cache slowness. A sibling in the same binary, `packaged_core_cancels_an_active_stream`, was `FLAKY 3/3` in the same run, which points at the packaged deterministic-loop harness as the shared locus.

**No timeout was raised, no override was added, and the test was not marked `#[ignore]`.**

### Flaky tests (passed, but recorded)

Five tests passed only on retry. They are counted in the 11517 passed and are **not** treated as failures, but they are recorded because a terminal aggregate proof should not silently absorb them:

```
   FLAKY 2/3 [   0.518s] ( 6069/11519) wcore-cli::harness_tui_flow tui_renders_the_chrome_and_every_tab_on_boot
   FLAKY 2/3 [   3.308s] ( 7395/11519) wcore-agent::dangerous_lease_e2e_test dangerous_expiry_cancels_production_streaming_bash_process_tree
   FLAKY 2/3 [   3.272s] ( 9149/11519) wcore-agent::dangerous_lease_e2e_test dangerous_expiry_reaches_bootstrapped_spawn_child
   FLAKY 3/3 [   2.550s] (11485/11519) wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream
   FLAKY 2/3 [   0.126s] (11497/11519) wcore-agent::bootstrap_file_watcher_test bootstrap_mounts_file_watcher_and_notifier_on_realfs_workspace
```

Full log: `20-56-evidence/test.log.gz` (sha256 of uncompressed = `3d8c3c9a…c187c`), 11945 lines. Also on the proof host at `hetzner-dsm:/root/f20-56/test.log`.

## Disposition: INCOMPLETE

This plan's terminal rule is unambiguous: *"Only when BOTH the build and the test run are green against that one exact SHA"* do the requirements complete; *"Any red, timeout, or truncated output leaves EVERY requirement incomplete with an explicit written disposition naming the failures."*

The build is green. **The test run is red — one hard failure and one timeout.** Therefore:

| Requirement | Status | Reason |
|-------------|--------|--------|
| F20-01 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-02 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-03 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-04 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-05 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-06 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-GATE-01 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |
| F20-GATE-02 | **INCOMPLETE** | Aggregate proof RED at `60565c53` |

`REQUIREMENTS.md` was **not** modified. No requirement was partially completed. No checkbox was ticked.

Both named blockers, restated compactly so they can be assigned without re-reading the log:

1. **`wcore-protocol` Desktop contract corpus drift** — 5 checked-in fixtures no longer match the real serializers (`adversarial/events/{fixture,schema,version}-mismatch.jsonl`, `events/ready.json`, `manifest.json`). Deterministic, 3/3 fail. Remediation hint from the test itself: `wcore-contract generate`. Needs a decision on whether the generator or the corpus is authoritative before anything is regenerated.
2. **`wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` hangs** — 180s hard kill, 3/3, on a warm-cache 96-core host with no per-test override. Same binary's `packaged_core_cancels_an_active_stream` was flaky 3/3 in the same run.

## Scope discipline observed

- **No native proof was run, claimed, dispatched, or waited on.** Windows/macOS native evidence is Phase 20A (Success Criteria 1 and 2) and appears nowhere in this SUMMARY as a claim.
- **No test was weakened, skipped, `#[ignore]`d, `#[allow]`ed, or deleted.** No timeout was raised. No override was added.
- **No lockfile was regenerated.** `--locked` passed cleanly on its own.
- **No push to `main`, no merge, no PR, no tag, no release, no issue closure.** The only remote write was pushing the work branch `plan/f20-unified-audit-repair` to `gh` so the proof host could fetch the exact SHA — explicitly authorized by this plan's dispatch.
- **No new PLAN file was created and no re-planning was done.**
- Diagnosis was held to a single round per failure; no third round was started on either.

## What a green would require

Not attempted here, recorded only so the next plan does not have to rediscover it: both failures must be resolved in source (or one of them explained as a genuine environment artifact with evidence, which the 3/3 determinism of each currently contradicts), then the identical two commands must be re-run against the new exact SHA and both come back green. Only then do F20-01..F20-06 / F20-GATE-01 / F20-GATE-02 complete.

## Self-Check: PASSED

- `20-56-SUMMARY.md` written at the planned path.
- `20-56-evidence/build.log.gz` and `20-56-evidence/test.log.gz` present, with uncompressed sha256 recorded and matching the values computed on the proof host.
- Proof-host HEAD re-verified equal to the pinned SHA (`60565c53…8eae`) and tree (`f873f15a…30ab`) with a clean working tree.
- `REQUIREMENTS.md` confirmed unmodified — all eight Phase 20 requirement checkboxes remain `- [ ]`.
