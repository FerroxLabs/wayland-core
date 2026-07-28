# RED-68 NOTES — running log (committed early per LANE-BRIEF §6b-i)

Lane `lane/red-68`, base `plan/f20-unified-audit-repair` @ `3cfc336f`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-red-68`.

Append-and-recommit after every measurement. Do not batch to the end.

---

## T+0 — established (all figures read back from real runs)

### The 68 are enumerated. So are the 81. They are NOT disjoint.

Source: GitHub Actions run `30403867920` on HEAD `189599ca`.

| leg | job | Summary line |
|---|---|---|
| Linux containerized | `90424728480` | `12820 tests run: 12752 passed (2 slow, 2 flaky), 68 failed, 50 skipped` |
| Windows self-hosted | `90424728470` | `12469 tests run: 12388 passed (2 slow, 1 flaky, 2 leaky), 81 failed, 116 skipped` |

The Linux job ran the suite **twice** — `nick-fields/retry@v3` wraps the nextest
invocation. Attempt 1: **69 failed**. Attempt 2: **68 failed**. The single test that
differs is
`wcore-sandbox backends::process_tree::linux_tests::required_live_descendant_teardown_before_workspace_cleanup`
— i.e. it is flaky across whole-suite attempts. The authoritative 68 is attempt 2.

**Overlap answer (the board question):**

| set | count |
|---|---|
| Linux 68 ∩ Windows 81 | **33** |
| Linux-only | 35 |
| Windows-only | 48 |
| **distinct tests failing across both platforms** | **116** |

So the two lists are neither identical nor disjoint. The problem is **116 distinct
failing tests**, not 149 and not 81.

Lists: `linux68.txt`, `win81.txt`, `overlap.txt`, `linux_only.txt`, `win_only.txt`,
plus `linux69.txt` (attempt 1, for the flaky diff).

### Instrument defect found and repaired IN THIS LANE (§6b-ii)

The obvious extractor for a nextest failure list is `grep 'FAIL ['`. **It
under-counts, silently, rc=0.** nextest emits a compound status token when a test
both fails and leaks a process: `FL+LK`. Measured on the ci-linux log:

| matcher | unique failing tests extracted | Summary says |
|---|---|---|
| `grep 'FAIL ['` | **66** | 68 |
| `extract-nextest-failures.py` | **68** | 68 |

The two it dropped:

```
wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind
wcore-exec-backend::fail_closed_matrix the_local_scan_finds_an_orphan_that_no_registry_remembers
```

This is the same class as every prior sighting: a matcher that answers an easier
question than the one you need, and reports absence rather than failing. It is
repaired rather than written up — `.planning/scripts/extract-nextest-failures.py`,
self-test with three assertions:

```
[ok] A1 known-positive: both FAIL and FL+LK extracted, PASS/LEAK/SKIP excluded
[ok] A2 known-negative: an all-PASS/LEAK/SKIP log yields zero failures
[ok] A3 the OLD matcher grep 'FAIL [' MISSES the FL+LK failure (old=1, new=2)
3 passed, 0 failed
```

A3 is the load-bearing one — A1 and A2 both pass against the broken matcher.

The extractor is additionally cross-checked against three independent oracles (the
three `Summary` lines) and reproduces **68 / 69 / 81 exactly**, via `--expect N`
which returns rc=1 on mismatch. It classifies by exclusion — any status token it has
never seen counts as a failure — so the next novel compound status fails loud instead
of vanishing.

### Second instrument defect, same session

`gh run view -R <repo> --job <id> --log` is intercepted by the `rtk` proxy, which
returns **`rtk: Run ID required` and rc=1** — the log never downloads. Working path:
`gh api /repos/<owner>/<repo>/actions/jobs/<id>/logs`. Recorded because the brief
already warns `rtk` silently filters `git log`; it also breaks `gh run view --job`.
`/bin/cat` and `/usr/bin/cat` differ on this Mac (`/usr/bin/cat` does not exist) —
another way a load-bearing command silently 127s inside a pipeline.

---

## T+1 — the CI container is missing three binaries the suite needs

The Linux CI image is built inline in `ci.yml`:

```
FROM rust:1.95-slim-bookworm
RUN apt-get install ... libdbus-1-dev libseccomp-dev libssl-dev libasound2-dev pkg-config mold ca-certificates git
```

**No `python3`, no `procps` (`ps`), no `bubblewrap`.** Read back from the job log,
lines 147-165. Failure messages name all three directly:

| missing | message | Linux failures |
|---|---|---|
| `python3` | `python3 must be available to materialise a hostile corpus: Os { code: 2, NotFound }` | 23 |
| `ps` | `could not run 'ps' to enumerate processes: No such file or directory` | 5 |
| `bwrap` | `required live bwrap must be installed and usable` / `sandbox backend fail_closed cannot enforce delegated read denial` | ~24 |

## T+1 — serial re-run on hetzner, per test, not per crate

hetzner has all three (`/usr/bin/python3`, `/usr/bin/ps`, `/usr/bin/bwrap`).
Re-ran the affected crates **serially** (`--test-threads 1 --no-fail-fast`) at this
lane's HEAD `82288335`:

| target | Summary |
|---|---|
| `-p wcore-protocol --test desktop_contract_corpus` | `15 tests run: 14 passed, 1 failed` |
| `-p wcore-exec-backend` | `124 tests run: 124 passed (1 leaky), 1 skipped` |
| `-p wcore-eval-scenarios` | `507 tests run: 507 passed, 5 skipped` |
| `-p wcore-sandbox` | `100 tests run: 100 passed, 2 skipped` |
| `-p wcore-swarm` | `150 tests run: 150 passed, 11 skipped` |
| `-p wcore-tools --test bash_sandbox_routing_test` | `18 tests run: 18 passed` |

**A crate total is not evidence about a specific test** — a crate can pass 100/100
without ever executing the test that failed in CI. Checked per test with
`.planning/scripts/verify-serial-outcome.py` (3-assertion self-test passes; A3 proves
the old "read the crate's N-passed summary" method would have cleared a test the
re-run never touched):

```
PASS_SERIAL=37   FAIL_SERIAL=1   ABSENT=30
```

The 30 ABSENT are `wcore-cli` and `wcore-agent` tests not in that batch — a second
serial batch is running for those. **They are NOT yet classified and must not be
counted as environment.**

## T+2 — the one that fails serially too: the Desktop contract corpus (HIGH)

`wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte`
fails on hetzner, serially, at HEAD. Not environment, not parallelism.

```
Desktop contract corpus drift: missing=[], extra=[],
drifted=["adversarial/events/fixture-mismatch.jsonl", "adversarial/events/schema-mismatch.jsonl",
         "adversarial/events/version-mismatch.jsonl", "events/ready.json", "manifest.json"]
```

Those five files are exactly the five that carry the contract descriptor. **No schema
file, no event file and no command file drifted** — the wire shape did not change;
the digests over it did.

`source_inputs_digest` recomputed outside cargo (`contract-source-digest.py`, mirrors
`generate::source_digest`; 3-assertion self-test passes):

| rev | computed | pinned | match |
|---|---|---|---|
| `5f74d559` (the authorized re-stamp) | `sha256:2517099…` | `sha256:2517099…` | **True** |
| `189599ca` (the CI run) | `sha256:e434c46…` | `sha256:2517099…` | False |
| `3cfc336f` / worktree | `sha256:3d760cf…` | `sha256:2517099…` | False |

So the re-stamp was correct when it landed and has been invalidated twice since.
`SOURCE_INPUTS` is **40 files, not the 20** a `head`-truncated read shows — and the
three that moved are:

```
crates/wcore-agent/src/output/protocol_sink.rs
crates/wcore-agent/src/bootstrap.rs
crates/wcore-cli/src/main.rs        <-- the LANE-BRIEF §6 shared-file fence
```

Seven commits from five lanes moved them since the re-stamp. One of them is
`bf959017 fix(24-c3): restore AgentBootstrap's own doc comment`.

**A restored doc comment moved a cryptographic wire-contract digest.** `main.rs` being
in the digest is worse still: the brief instructs *every* lane to make additive edits
to that exact file, so the guard is designed to be re-reddened by the workflow itself.

I did NOT run `wcore-contract generate` (brief §0). A fenced seam request goes in the
report.

## Still to establish

- [ ] Second serial batch (`wcore-cli --lib`, `portability_hostile_corpus`,
      `f14_sigkill_recovery`, `sandbox_activeness`) — the 30 currently ABSENT.
- [ ] Windows: whether `--json-stream` really emits no `ready`, measured with a
      probe that isolates config properly and reads stderr.
- [ ] Rank the real defects by customer impact.
