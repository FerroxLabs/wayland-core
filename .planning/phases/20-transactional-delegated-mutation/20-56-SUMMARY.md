---
phase: 20-transactional-delegated-mutation
plan: "56"
type: execute
status: complete
completed: 2026-07-25
disposition: complete
superseded_disposition: incomplete   # the RED run at 60565c53, preserved verbatim below
requirements_completed:
  - F20-01
  - F20-02
  - F20-03
  - F20-04
  - F20-05
  - F20-06
  - F20-GATE-01
  - F20-GATE-02
requirements_left_incomplete: []
# The one exact SHA the ACCEPTED (green) result is bound to
source_sha: 01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb
source_tree: 4a5247ca804a88c5fc621402d5e55a3dab10e8a5
source_branch: plan/f20-unified-audit-repair
proof_host: hetzner-dsm
proof_checkout: /root/wayland
proof_host_head: 01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb
proof_host_tree: 4a5247ca804a88c5fc621402d5e55a3dab10e8a5
proof_host_dirty_paths: 0
aggregate_build: green
aggregate_test: green
evidence:
  - path: .planning/phases/20-transactional-delegated-mutation/20-56-evidence/build-01a5b0ae-GREEN.log.gz
    sha256_uncompressed: 216d771af391d3eba344747b0e65f725013443f2c0083b5a2c9f983d236aac3a
  - path: .planning/phases/20-transactional-delegated-mutation/20-56-evidence/test-01a5b0ae-GREEN.log.gz
    sha256_uncompressed: 9f32a6f6a7cd7940bfda8bd6c268abf66c0aff2a0cdbb53403e44adfe8e53ff3
# The superseded RED run at 60565c53, retained
superseded_run:
  source_sha: 60565c53165024366a7ea93ddee852c7e27a8eae
  source_tree: f873f15aeb684d0cb49280f5d62a020fec9230ab
  aggregate_build: green
  aggregate_test: RED
  evidence:
    - path: .planning/phases/20-transactional-delegated-mutation/20-56-evidence/build-60565c53-RED.log.gz
      sha256_uncompressed: 8b61c422bdfe97b352a693aedd1ed0e535e61f7d44fe1a405ff90c4bbc4077db
    - path: .planning/phases/20-transactional-delegated-mutation/20-56-evidence/test-60565c53-RED.log.gz
      sha256_uncompressed: 3d8c3c9a2324a338288ee62570055f566a82cbbe876ef5ca9c244c9e669c187c
---

# Phase 20 Plan 56: Aggregate Hetzner Proof — COMPLETE (both green at `01a5b0ae`)

> **SUPERSEDING NOTE (2026-07-25, plan 20-57).** The original disposition of this
> plan was INCOMPLETE against SHA `60565c53`: build green, test RED (1 failed,
> 1 timed out). **That record is preserved verbatim in the sections below and
> nothing in it has been deleted.** Both named blockers were subsequently
> diagnosed and fixed at their cause, and the identical two commands were re-run
> against the new exact SHA `01a5b0ae`, where **both came back GREEN**. The
> closeout is recorded in [Closeout](#closeout-2026-07-25--both-green-at-01a5b0ae)
> at the end of this file, which is the authoritative disposition. Everything
> between here and that section describes the superseded RED run.

**SUPERSEDED — original text follows.** The aggregate `--locked --workspace --all-features` build is GREEN against the exact pinned SHA `60565c53`, but the aggregate `nextest --profile ci --no-fail-fast` run is RED: 11517 passed, 1 failed, 1 timed out, 48 skipped (exit 100). Per this plan's own terminal rule, EVERY Phase 20 requirement (F20-01..F20-06, F20-GATE-01, F20-GATE-02) is left INCOMPLETE and this explicit incomplete disposition is recorded. Nothing was weakened, skipped, `#[ignore]`d, or deleted to reach green.

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

Full log: `20-56-evidence/build-60565c53-RED.log.gz` (sha256 of uncompressed = `8b61c422…4077db`).

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

Full log: `20-56-evidence/test-60565c53-RED.log.gz` (sha256 of uncompressed = `3d8c3c9a…c187c`), 11945 lines. Also on the proof host at `hetzner-dsm:/root/f20-56/test.log`.

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
- `20-56-evidence/build-60565c53-RED.log.gz` and `20-56-evidence/test-60565c53-RED.log.gz` present, with uncompressed sha256 recorded and matching the values computed on the proof host.
- Proof-host HEAD re-verified equal to the pinned SHA (`60565c53…8eae`) and tree (`f873f15a…30ab`) with a clean working tree.
- `REQUIREMENTS.md` confirmed unmodified — all eight Phase 20 requirement checkboxes remain `- [ ]`.

---

# Closeout 2026-07-25 — both green at `01a5b0ae`

**Everything above this line describes the superseded RED run at `60565c53` and is retained
unaltered. This section is the authoritative disposition of plan 20-56.**

Both named blockers were diagnosed at their cause and fixed. The identical two commands were
re-run against the new exact SHA and **both came back green**. F20-01..F20-06, F20-GATE-01 and
F20-GATE-02 are now complete, bound to that one SHA.

## The one exact SHA (accepted)

| Field | Value |
|-------|-------|
| `source_sha` | `01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb` |
| `source_tree` | `4a5247ca804a88c5fc621402d5e55a3dab10e8a5` |
| branch | `plan/f20-unified-audit-repair` |
| subject | `test(cli): give the F04 repeatability run its measured time budget` |

Hetzner was verified on it **before anything ran**:

```
$ ssh hetzner-dsm 'cd /root/wayland && git rev-parse HEAD'
01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb
$ ssh hetzner-dsm 'cd /root/wayland && git rev-parse HEAD^{tree}'
4a5247ca804a88c5fc621402d5e55a3dab10e8a5
$ ssh hetzner-dsm 'cd /root/wayland && git status --short | wc -l'
0
```

Host HEAD, host tree and cleanliness all match the pinned SHA exactly. Toolchain unchanged:
`cargo 1.95.0 (f2d3ce0bd 2026-03-21)`, `rustc 1.95.0 (59807616e 2026-04-14)`,
`cargo-nextest 0.9.137 (75ddba7e9 2026-05-26)`, 96 cores.

## Repair 1 — contract corpus drift (commit `48b9518b`)

`checked_corpus_matches_real_serializers_byte_for_byte` failed with
`missing=[], extra=[], drifted=[5 files]`. Because `missing` and `extra` were both empty, no
fixture had been added or removed — only content had moved. Regeneration was performed with the
generator named by the test itself (`cargo run -p wcore-protocol --bin wcore-contract -- generate`,
run on the Linux build host), and **the resulting diff was inspected key-by-key before being
accepted**, because regenerating a Desktop-facing contract corpus otherwise silently ratifies
whatever changed.

**What actually changed: exactly two hex provenance digests, and nothing else.**

| Field | Before | After |
|-------|--------|-------|
| `source_inputs_digest` | `sha256:7bbf7f34…92cb62` | `sha256:f6032969…b67c5cd` |
| `fixture_digest` | `sha256:f71d2851…5c0654` | `sha256:794bc0b8…6b8500a` |
| `schema_digest` | `sha256:e5d1744a…ff2e54` | **unchanged** |

Verified by flattening every JSON path in all five files before and after:

| File | Delta |
|------|-------|
| `manifest.json` | `fixture_digest` + `source_inputs_digest` only |
| `events/ready.json` | `contract.fixture_digest` + `contract.source_inputs_digest` only |
| `adversarial/events/version-mismatch.jsonl` | same two fields only |
| `adversarial/events/schema-mismatch.jsonl` | same two fields only |
| `adversarial/events/fixture-mismatch.jsonl` | `contract.source_inputs_digest` **only** |

**Why this is not a protocol break.** Zero keys added, removed or renamed in any of the five
files. No event `type` changed — all five are still `ready`. No version bump: `contract.major` /
`contract.minor` remain `1` / `8` everywhere (`version-mismatch.jsonl`'s `major: 2` is its
deliberate adversarial forgery and is unchanged). Counts unchanged: 18 commands, 49 events,
3 child types, 151 fixtures. Every capability and subcontract value unchanged. `schema_digest`
is byte-identical, which is the direct statement that **the Desktop-facing wire schema did not
change**. And `fixture-mismatch.jsonl` shows *only* `source_inputs_digest` moving — its
`fixture_digest` stays pinned at the `ffff…` sentinel that `insert_negotiation_fixtures`
deliberately forces. That asymmetry is what a structurally correct regeneration looks like; a
blanket overwrite would have moved both fields there too.

**Cause.** `source_inputs_digest` hashes the 40 `.rs` files listed in `SOURCE_INPUTS`
(`crates/wcore-protocol/src/contract/spec.rs:833`), which span ten crates — not just
`wcore-protocol`. Exactly one of those 40 moved since the previous re-pin `6937ef61`:
`crates/wcore-cli/src/main.rs`, in `bf3b7421 fix(swarm): raise the bounded test-thread and
runtime-worker stacks`, which added `.thread_stack_size(8 * 1024 * 1024)` to the tokio runtime
builder plus its rationale comment. That has no wire surface at all. Because `events/ready.json`
and the three adversarial fixtures embed the contract descriptor, and `fixture_digest` is
computed over the fixture set that *includes* `ready.json`, the source-provenance change
propagates into `fixture_digest` and then into `manifest.json` — producing exactly the five
observed files. This is the same benign mechanism `6937ef61` already recorded once.

Verdict: **legitimate provenance re-pin consequent on landed work.** Regenerated and committed.

## Repair 2 — F04 repeatability timeout (commit `01a5b0ae`)

`packaged_f04_run_is_repeatable_and_content_addressed` was killed at `180.004s` on all three
attempts. The question — genuine hang, or genuinely slow — was answered by measurement, not
inspection.

**It is SLOW, and it PASSES.** Run alone on the proof host, twice:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 188.43s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 188.03s
Elapsed (wall clock) 3:08.03   User 185.02s   System 3.34s
```

Two independent runs 0.4s apart in cost. `user 185.02s / wall 188.03s` ≈ **98% of one core for
the entire run** — the process is computing continuously, not blocked. Process sampling at 2s
intervals confirms it directly: the test binary sits at 96–101% CPU from `t=2s` to `t=186s`, and
each of the two spawned `wayland-core` children appears in exactly one sample (age 0–1s), i.e.
both packaged runs complete in under ~2s. No child stalls. Every wait in the harness is already
bounded — 30s scenario guard (`tokio::time::timeout(scenario.max_total_time, drive)`), 5s child
reap (`reap_child`), 5s cgroup drain (`wait_empty_and_remove`) — so there is no unbounded wait to
deadlock on.

Two candidate explanations were tested and eliminated:
- **Contention.** Rejected: it was scheduled last (`11519/11519`) on an idle 96-core host in the
  RED run, and it reproduces at 188s completely alone.
- **The 980 MiB debug artifact seal.** Rejected by direct measurement: `cp` of the binary is
  0.68s and `sha256sum` is 0.68s, so the whole seal/verify path is ~2s per run, not ~90s.

So: **this test needs ~188s, and the global CI budget kills at 90s × 2 = 180s.** It was
terminated roughly 4% short of finishing. That is the entire failure.

Fix: a per-test override in `.config/nextest.toml`, `slow-timeout = { period = "120s",
terminate-after = 4 }` → 480s hard kill, **2.55× the measured 188s**. `period` is deliberately set
*below* the measured runtime rather than above it, so nextest still prints the "running for over
120 seconds" notice and the cost stays visible in every log instead of being silently absorbed.
The **global** slow-timeout is untouched, no assertion was changed, and nothing was skipped,
`#[ignore]`d or `#[allow]`ed.

Noted, not chased (out of scope, one line each as instructed): 185s of single-core CPU inside the
test process for two ~2s packaged runs is a real cost that deserves its own investigation; and the
`default` nextest profile still runs this test on the 30s × 2 budget.

## Result 1 (re-run) — aggregate build: GREEN

```
cargo build --locked --workspace --all-features
```

- **Exit code 0.** `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 19.90s`
  (fast because the target dir was warm from the diagnostic runs on the same host; no `.rs` file
  changed between `60565c53` and `01a5b0ae`, only `.config/nextest.toml` and five corpus JSON files).
- Zero `error` lines. The one warning is the same non-ours `imap-proto v0.10.2` future-incompat note.
- **`--locked` did NOT fail.** No lockfile was regenerated, touched, or bypassed.

Full log: `20-56-evidence/build-01a5b0ae-GREEN.log.gz`
(sha256 uncompressed `216d771af391d3eba344747b0e65f725013443f2c0083b5a2c9f983d236aac3a`).

## Result 2 (re-run) — aggregate test: GREEN

```
cargo nextest run --profile ci --no-fail-fast
```

- **Exit code 0.**
- Started `2026-07-25 06:39:56 UTC`, ended `06:45:48 UTC`.

### Observed counts (actual, not asserted)

```
Starting 11519 tests across 469 binaries (48 tests skipped)
Summary [ 194.331s] 11519 tests run: 11519 passed (1 slow, 3 flaky), 48 skipped
```

| Metric | Observed | vs. RED run |
|--------|----------|-------------|
| tests run | **11519** | same |
| passed | **11519** | 11517 → 11519 |
| failed | **0** | 1 → 0 |
| timed out | **0** | 1 → 0 |
| skipped | **48** | same |
| flaky (passed on retry) | 3 | 5 → 3 |

The enumerated total is reported as observed. It is unchanged at 11519 because neither repair
added or removed a test — one is a data regeneration, the other a nextest budget.

Both previously-failing tests now pass:

```
PASS [   0.295s] ( 8231/11519) wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte
SLOW [ 187.062s] (11519/11519) wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed
```

`187.062s` under the full parallel suite against a measured-alone `188.0–188.4s` confirms the
diagnosis exactly, and the `SLOW` marker confirms the deliberate `period = 120s` visibility choice
is working as intended.

### Flaky tests (passed, but recorded)

Three tests passed only on retry. They are counted in the 11519 passed and are **not** failures,
but are recorded so a terminal proof does not silently absorb them:

```
FLAKY 3/3 [   3.204s] ( 9355/11519) wcore-agent::dangerous_lease_e2e_test dangerous_expiry_reaches_bootstrapped_spawn_child
FLAKY 2/3 [   0.057s] (10115/11519) wcore-swarm worktree::tests::linux::status_output_cap_kills_git_descendant
FLAKY 2/3 [   3.336s] (10174/11519) wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream
```

Down from five in the RED run. `packaged_core_cancels_an_active_stream` — flagged in the RED run
as a possible shared-locus signal with the F04 timeout — remains flaky and passing; it was not
touched, and the F04 diagnosis found no shared defect to fix (F04 was never hung).

Full log: `20-56-evidence/test-01a5b0ae-GREEN.log.gz`
(sha256 uncompressed `9f32a6f6a7cd7940bfda8bd6c268abf66c0aff2a0cdbb53403e44adfe8e53ff3`),
11598 lines. Also on the proof host at `hetzner-dsm:/root/f20-57/test.log`.

## Disposition: COMPLETE

The plan's terminal rule is satisfied: **both** the build and the test run are green against one
exact SHA.

| Requirement | Status | Bound to |
|-------------|--------|----------|
| F20-01 | **COMPLETE** | `01a5b0ae` |
| F20-02 | **COMPLETE** | `01a5b0ae` |
| F20-03 | **COMPLETE** | `01a5b0ae` |
| F20-04 | **COMPLETE** | `01a5b0ae` |
| F20-05 | **COMPLETE** | `01a5b0ae` |
| F20-06 | **COMPLETE** | `01a5b0ae` |
| F20-GATE-01 | **COMPLETE** | `01a5b0ae` |
| F20-GATE-02 | **COMPLETE** | `01a5b0ae` |

`REQUIREMENTS.md` updated: those eight checkboxes ticked and the traceability row set to
`Complete @ 01a5b0ae`. The twelve `REQ-native-r1 … r12` rows were **not** touched — they are
Phase 20A.

## Scope discipline observed (closeout)

- **No native proof was run, claimed, dispatched, or waited on.** Windows/macOS native evidence is
  Phase 20A and appears nowhere here as a claim.
- **No test was weakened, skipped, `#[ignore]`d, `#[allow]`ed, or deleted.** No assertion was
  changed. The **global** slow-timeout was not raised.
- **No lockfile was regenerated.** `--locked` passed cleanly on its own.
- **No push to `main`, no merge, no PR, no tag, no release, no issue closure.** The only remote
  write was pushing the work branch `plan/f20-unified-audit-repair` to `gh` so the proof host could
  fetch the exact SHA — explicitly authorized.
- **No new PLAN file was created and no re-planning was done.**
- Diagnosis was held to the two-round bound on each failure; no third round was started on either.

## Self-Check (closeout): PASSED

- All four evidence files present at `.planning/phases/20-transactional-delegated-mutation/20-56-evidence/`
  (`build-01a5b0ae-GREEN.log.gz`, `test-01a5b0ae-GREEN.log.gz`, `build-60565c53-RED.log.gz`,
  `test-60565c53-RED.log.gz`). The RED pair was renamed, not deleted.
- Uncompressed sha256 of both green logs recomputed locally after transfer and matched the values
  computed on the proof host byte for byte.
- Proof-host HEAD re-verified equal to `01a5b0ae…1b9bb` and tree `4a5247ca…0e8a5`, working tree clean,
  **before** the build and test commands ran.
- Both repair commits exist on the branch: `48b9518b` (corpus re-pin), `01a5b0ae` (nextest budget).
