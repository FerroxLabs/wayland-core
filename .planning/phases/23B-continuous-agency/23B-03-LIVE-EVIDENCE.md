# 23B-03 — LIVE EVIDENCE (F23-06, the persistent incremental repository index)

Every number below was produced by **running the shipped `wayland-core` binary
against this real workspace** — 3,603 in-scope files across 50+ crates,
including the 320 KB and 1.2 MB single files — and captured into a drive log
under `evidence/`. Nothing here is asserted from prose. The gates read the
**captured drive logs**, never this file, because a word count over a table
cannot tell a measured number from a typed one.

Each leg was driven with a **nonce the caller generated at run time**, echoed
in the terminal `F23_03_DRIVE=PASS` marker, so a stale log from an earlier run
cannot satisfy a later check. Each binary's own `--build-info` source SHA was
asserted equal to the commit under test **before any measurement was taken**:
a measurement against an unidentifiable binary is not a measurement.

---

## 0. Leg status, stated plainly

| Platform | Binary provenance | Drive | Disposition |
|---|---|---|---|
| Linux (`hetzner-dsm`) | built on the host at the commit under test | `evidence/23B-03-linux-drive.log` | **PASS** |
| Windows (`SeanDesktop`, native msvc) | built on the host at the commit under test | `evidence/23B-03-windows-drive.log` | **PASS** |
| macOS (this Mac, arm64) | would be the CI `build` job artifact `wayland-core-aarch64-apple-darwin` | *no log — the leg did not run* | **NOT ACHIEVED** (§6) |

---

## 1. Thresholds and the order they were chosen in

**Every threshold in this plan was chosen AFTER its measurement, and none was
widened after a failure.** No gate in this plan reddened and was then
loosened; where a measurement disappointed, the number is reported as it is.

| Gate | Threshold | Chosen | Measured (Linux) |
|---|---|---|---|
| Warm start reads **zero** files | `read == 0` exactly | **BEFORE** — it is the plan's definition of incrementality, not a tuning knob | 0 / 0 / 0 |
| Unchanged files not re-extracted | `extracted <= files touched` | **BEFORE** — same reason | 0 surplus on all 5 mutations |
| Gitignored nonce in store bytes | `== 0` occurrences | **BEFORE** — it is a security property | 0 |
| Retrieval `precision@1` (unit corpus) | `>= 0.90` | **AFTER**, from a measured 1.00 | 1.00 |
| Retrieval `recall@10` (unit corpus) | `>= 0.95` | **AFTER**, from a measured 1.00 | 1.00 |
| Cold build / warm start / size / latency | **no pass-fail threshold set** | see the note below | recorded, §2 |

**On the four perf gates specifically.** F23-06 asks for warm-start, size and
latency to be *measured and recorded*. They are, three samples each, all
samples published. I did **not** invent a pass-fail number for them, and that
is deliberate: a first-ever measurement has no prior to be a regression
against, and a threshold set from a single session's number on a 96-core
shared host would be a number invented to be passed rather than a bound
anything was designed to meet. What IS gated is the property that makes the
warm number meaningful — that a warm start opens **zero** files — and that
gate is absolute and was chosen before measuring. These figures are the
baseline a later phase can ratchet against; recording them as a "gate met"
would be the engineered green this plan exists to avoid.

---

## 2. Linux — `hetzner-dsm`, 96 cores

Commit under test `b33827d3a2b8ebc70a17a16a556cf1bde0e4228b`; run nonce
`d3b14061fc7a3735`; `F23_03_PROVENANCE=ok platform=linux sha=b33827d3…`.

Corpus: `/root/wayland-23B-03` — **3,603 records, 37,868 symbols**.

### Cold build — all three samples

| Sample | Wall (ms) | Files read | Records | Symbols |
|---:|---:|---:|---:|---:|
| 1 | 1535 | 3603 | 3603 | 37868 |
| 2 | 1541 | 3603 | 3603 | 37868 |
| 3 | 1504 | 3603 | 3603 | 37868 |

### Warm start — all three samples

| Sample | Wall (ms) | Files read | Files re-extracted |
|---:|---:|---:|---:|
| 1 | 109 | **0** | **0** |
| 2 | 113 | **0** | **0** |
| 3 | 107 | **0** | **0** |

**Warm : cold ratio ≈ 0.071** (109 / 1535). The warm number is a *refresh* of
an unchanged store, and it is credible precisely because it read nothing: the
ratio is not a stopwatch claim about a faster rebuild, it is the cost of a
scope walk with no file opened behind it.

### On-disk size

66,555,904 bytes on all three samples (≈ 63.5 MiB for 3,603 files and 37,868
symbols, including the full text stored for the exact-search fallback).

Measured, and worth recording: **immediately after a cold build, before the
write-ahead log is folded back, the same store reports 133,366,096 bytes** —
2× the steady-state figure. A size gate that sampled the transient would be
measuring when it happened to look. `IndexStore::refresh` now checkpoints the
WAL before returning for exactly this reason.

### Query latency — 20-query fixed set, one sample each

`p50 = 5,810 µs`, `p95 = 6,159 µs`, n = 20.
All samples (µs): 5485 5513 5539 5633 5643 5648 5714 5727 5797 5810 5909 5942
5955 5968 6023 6029 6093 6106 6159 7171.

The exact-search fallback path is an order of magnitude slower — `51,601 µs`
measured separately on the `=> {` query — because it is a full scan of stored
text by `instr()`. That is the price of answering a query full text cannot
serve at all, and it is recorded rather than hidden.

### Retrieval quality — through the shipped binary, full workspace corpus

`precision@1 = 0.8125`, `recall@10 = 1.0000` over 16 queries.

Three of sixteen concept-shaped queries did **not** put the expected file
first, and the top hits are recorded:

| Query | Expected | Actual top hit |
|---|---|---|
| `content hash invalidation` | `crates/wcore-repomap/src/store.rs` | `crates/wcore-agent/src/orchestration/anvil/forge.rs` |
| `worktree identity` | `crates/wcore-repomap/src/scope.rs` | `.planning/phases/20-…/20-06-PLAN.md` |
| `bm25 full text` | `crates/wcore-repomap/src/search.rs` | `crates/wcore-memory/src/retrieve.rs` |

All three were found within the top 10 — recall is perfect — but BM25 alone
ranks a prose-heavy planning document or a doc-comment above the definition.
**This is the case the OPTIONAL semantic layer exists to fix, and that layer
is not built.** It is recorded as finding 23B-03-M1 rather than smoothed away
by trimming the corpus to the queries that scored well.

### Incremental mutations — all five, driven for real

Scratch repository materialised from `git archive HEAD` and `git init`-ed
(3,608 files), never the measurement checkout.

| Mutation | Counter | Files re-extracted | Surplus re-extraction |
|---|---|---:|---:|
| add | `added=1` | 1 | **0** |
| edit | `changed=1` | 1 | **0** |
| delete | `deleted=1` | 0 | **0** |
| rename (content unchanged) | `renamed=1` | **0** | **0** |
| branch switch | `added=1` | 1 | **0** |

Scope identity after the switch:
`commit=484186fb… ref=refs/heads/f23-drive-d3b14061fc7a3735 gitdir=…/clone/.git`
— the recorded identity moved with HEAD, and the 3,603 unchanged records were
not touched.

### Secret isolation — against the store's own bytes

```
F23_03_STORE_CONTROL_OCCURRENCES=1     <- the control marker in an INDEXED file
F23_03_STORE_NONCE_OCCURRENCES=0       <- the secret in a GITIGNORED file
```

The control line is the load-bearing half. Without it, a store that held no
content at all would satisfy the zero-occurrence assertion vacuously. Its
presence proves the store *does* contain file text and *could* have contained
the secret — and does not.

### Fallback and staleness

```
F23_03_FALLBACK_REPORTED=true
F23_03_STALENESS_REPORTED=true
F23_03_VERIFY=agrees=false exit=6
```

Staleness is asserted **before and after**: the hit reported
`content_stale=false` on the freshly-indexed file and `content_stale=true`
after the edit. A hit that had always been stale would prove nothing, so the
before-assert is what makes the after-assert mean something. `verify`
independently exited 6 over the same drifted tree.

---

## 3. Gates that were proved able to go RED

A gate that cannot fail is worse than no gate. Each of the following was made
to fail on purpose, on real hardware, and the failure output was read.

| Gate | How it was made to fail | Observed |
|---|---|---|
| `reopening_an_unchanged_store_reads_no_files` | replaced the size+mtime skip with `if false` on `hetzner-dsm` | FAILED: `files_read: 3` vs expected 0 |
| `a_nonce_in_a_gitignored_file_is_absent_from_the_store_bytes` | set `respect_gitignore = false` | FAILED: "a run-time nonce planted in a gitignored file was found in the store's own bytes" |
| `renaming_…_reuses_its_hash_and_re_extracts_nothing` | disabled rename detection | FAILED: `added: 1, deleted: 1, renamed: 0, files_extracted: 1` |
| the Windows ssh gate shape | asserted a file that does not exist | exit **92**, propagated through `exit $LASTEXITCODE` |
| the Windows ssh gate shape | `-E 'test(definitely_no_such_test)' --no-tests=fail` | exit **4** |

After each red proof the modified file was restored from a backup copy and the
tree confirmed clean. **No gate command in this plan is a pipeline into a
filter**; every remote leg redirects to a log, captures `rc` on the next line,
asserts on `rc` first, and only then reads the log for the nonce marker.

Two self-passing shapes were caught *in this plan's own work* and fixed rather
than tolerated:

1. **The absent-token probe was present by construction.** The retrieval
   fallback test used a literal "no such token"; the corpus is the crate's own
   tree, so the literal was sitting in the test file and the index found it.
   Fixed by generating the token at run time — not by weakening the assertion.
2. **A field extractor returned empty for the first field on a line.** The
   driver's anchored `sed` regex demanded whitespace before the key, so
   `agrees=` — which sits immediately after the already-consumed space —
   silently came back blank on a correct `verify`. Caught because the exit
   code said 6 while the parsed field said nothing.

---

## 4. Cargo gates

| Gate | Host | Result |
|---|---|---|
| `cargo fmt --all -- --check` | this Mac | clean |
| `cargo clippy -p wcore-repomap --all-targets -- -D warnings` | `hetzner-dsm` | clean (2 findings **fixed**, not allowed: two collapsible `if`s) |
| `cargo clippy --workspace --all-targets -- -D warnings` | `hetzner-dsm` | clean |
| `cargo nextest run -p wcore-repomap` | `hetzner-dsm` | **58 run, 58 passed**, 0 retries consumed |
| `cargo nextest run -p wcore-repomap -p wcore-tools` | `hetzner-dsm` | **1244 run, 1244 passed**, 3 skipped, 0 retries consumed |
| `cargo nextest run -p wcore-cli` | `hetzner-dsm` | **2177 run, 2173 passed, 4 failed** — see below |
| `cargo nextest run -p wcore-repomap` | **SeanDesktop, native Windows** | **57 run, 57 passed** (58th is `#[cfg(unix)]`) |
| `cargo hakari verify` | — | **NOT RUN**: `cargo-hakari` is not installed on `hetzner-dsm` and is not a CI step. Reported as not run rather than as a pass. |

**The four `wcore-cli` failures are pre-existing and were PROVED so, not
assumed.** `child_authority_corpus::{corpus_time, corpus_token, corpus_cost,
corpus_depth}` fail identically at the untouched base commit
`32e2f57d09fe4b287e513081862217dc9daa5901`, on a tree asserted by
`test ! -f crates/wcore-repomap/src/store.rs` not to contain this plan's work.
Their own message says so: *"a production file forwards a child-supplied
budget override: crates/wcore-agent/src/spawner.rs. This is EXPECTED from
2026-07-27."* They belong to the F21-02 child-budget work, not to this plan.

**The workspace lock claim was verified, not trusted.**
`git diff -U0 Cargo.lock | grep -E '^[+-]name = '` returns **no output**: the
diff adds two dependency edges to `wcore-repomap` and **zero** `[[package]]`
entries. `rusqlite` 0.32 (`bundled`, which ships FTS5) and `sha2` 0.10 were
already resolved for the workspace.

**The isolation rule holds.**
`grep -cE '^wcore-' crates/wcore-repomap/Cargo.toml` returns **0**.
`wcore-memory`'s hybrid retriever was mirrored as a *pattern* — BM25 ordered
ascending, joined on rowid, fused at k = 60 — and is not imported.

---

## 5. Windows — `SeanDesktop`, native msvc

**PASS.** `WIN_DRIVE_RC=0`, and the log carries
`F23_03_DRIVE=PASS platform=windows nonce=8ed4d1215a01c1f4` — the nonce the
caller generated for this run. Commit under test
`3cf304e90aaee21ed8993ee14163501ee020a81a`, asserted against the binary's own
`--build-info` before any measurement.

The remote command string ends in an explicit `exit $LASTEXITCODE` and is
**never** piped into a filter; the status was captured on the next line and
asserted before the log was read for the marker.

Corpus: `C:\ferrox-win` — **3,609 records, 37,868 symbols** (six more files
than the Linux checkout; the symbol count is identical, so the difference is
non-source).

### Cold build / warm start / size

| Sample | Cold (ms) | Warm (ms) | Warm reads | Warm re-extracts | Store bytes |
|---:|---:|---:|---:|---:|---:|
| 1 | 1576 | 151 | **0** | **0** | 66,592,768 |
| 2 | 1479 | 157 | **0** | **0** | 66,592,768 |
| 3 | 1522 | 157 | **0** | **0** | 66,592,768 |

**Warm : cold ratio ≈ 0.099.** Store size is within 37 KB of Linux for six
more files — the format is platform-stable.

### The one Windows-specific perf observation, reported rather than smoothed

The **first** cold build on this host, in the earlier run against
`decbca2b…`, took **24,824 ms** — 16× the 1,461 ms its own second sample
recorded moments later. Every subsequent sample, in both runs, landed at
1,419–1,576 ms. The corpus, the binary and the code were identical across
those samples, so the 24.8 s is the OS file cache being cold over 3,609
files, not the index. It is recorded because a reader planning around a
cold-start budget on Windows needs the 24.8 s number, not the 1.5 s one. It
is not a defect and no threshold was moved because of it.

### Query latency

`p50 = 5,016 µs`, `p95 = 5,350 µs`, n = 20 — slightly **faster** than Linux
(5,810 / 6,159), on a much smaller machine. All samples (µs): 4617 4664 4703
4797 4820 4851 4877 4918 4964 5016 5063 5150 5171 5193 5224 5251 5325 5336
5350 6057.

### Retrieval quality

`precision@1 = 0.8125`, `recall@10 = 1.0000` — **identical to Linux, query
for query**, including which three queries lose top-1. Ranking is
platform-stable, which is the thing that would have been easiest to get
wrong and is worth having measured rather than assumed.

### Incremental mutations

All five PASS with `unchanged_reextracted=0`, against a 3,610-file scratch
tree with 3,605 records:

| Mutation | Counter | Re-extracted |
|---|---|---:|
| add | `added=1` | 1 |
| edit | `changed=1` | 1 |
| delete | `deleted=1` | 0 |
| rename | `renamed=1` | **0** |
| branch switch | `added=1` | 1 |

### Path representation — the class this platform was expected to break on

`cargo nextest run -p wcore-repomap` on this host: **57 run, 57 passed**
(the 58th is `#[cfg(unix)]`). That includes
`stored_paths_round_trip_for_non_ascii_and_deeply_nested_names`, which stores
and then looks up `src/ünïcode/módulo.rs` and an 18-level nested path through
the real store, by canonical key and by native separator. The
normalise-on-both-operands rule held.

One representation artefact **is** visible and is recorded as finding
23B-03-M2: the scope fingerprint carries the verbatim extended-length prefix,
slash-normalised —
`gitdir=//?/C:/Users/seand/AppData/Local/Temp/f23idx-…/clone/.git`. It is
self-consistent (both operands pass through the same function, and the
branch-switch comparison worked), but it is ugly and would not compare equal
to a fingerprint produced without `fs::canonicalize`.

### Staleness, verify and secret isolation

```
F23_03_STALENESS_REPORTED=true
F23_03_VERIFY=agrees=false exit=6
F23_03_STORE_CONTROL_OCCURRENCES=1
F23_03_STORE_NONCE_OCCURRENCES=0
F23_03_FALLBACK_REPORTED=true
```

### The defect this leg found, and how the gate behaved

The plan predicted Windows would be the platform that finds a defect, and it
was — in the driver rather than in the product. The **first** Windows run
exited **69** with:

```
/usr/bin/tar: Cannot connect to C: resolve failed
FATAL: tar -xf exited 128
```

The `tar` on PATH is git-for-Windows' GNU tar, which parses an `-f` argument
containing a colon as a remote `host:path` spec and tries to reach a host
literally named `C`. Any drive-lettered path is unusable with `-f` without
`--force-local`.

**What matters as much as the bug is that the gate behaved correctly**: it
exited non-zero, emitted **no** PASS marker, and the caller's
`grep -qF "F23_03_DRIVE=PASS … nonce=…"` found nothing — even though 40 lines
of perfectly good measurements had already been printed above the failure. A
pipeline-into-a-filter gate would have greened on those surviving lines.
Fixed by switching the Windows port to `git archive --format=zip` plus
`Expand-Archive`, which avoids the class rather than patching one instance.

---

---

## 6. macOS — this Mac, arm64

**NOT ACHIEVED.** Not a pass and not a fail: the leg did not run, and no
macOS row in this document is filled from anything else.

This is stated plainly rather than dressed up, and the earlier revision's
mistake is explicitly not repeated: **no macOS number here was obtained by
grepping an evidence file this executor wrote.** There are no macOS numbers.

### Why the plan's own route was unavailable

The plan decides "the macOS leg builds its own binary on this Mac, through
`scripts/f23-macos-binary.sh`, which 23B-01 owns and this plan consumes
unchanged", and instructs: *"If `scripts/f23-macos-binary.sh` is absent
because 23B-01 did not land it, STOP and record that as a blocking dependency
rather than improvising a second resolver."*

Measured: `scripts/f23-macos-binary.sh` **does not exist**. `23B-01-SUMMARY.md`
says so in its own deviations — *"`scripts/f23-macos-binary.sh` was NOT
written… The phase's controlling instruction forbids running Cargo on the Mac.
I honoured the controlling instruction and escalated the conflict."*
`23B-02-SUMMARY.md` records the same conflict, unchanged. This lane's
controlling instruction forbids Cargo on the Mac too (`cargo fmt --all --
--check` excepted, and that was run and is clean).

### The route that IS correct, and exactly how far it got

`.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md` is right, and I verified its
mechanism rather than taking it on trust:

- `.github/workflows/ci.yml:484-490` uploads `wayland-core-${{ matrix.target }}`
  containing `target/<target>/release/wayland-core`, `if-no-files-found: error`,
  `retention-days: 14`, from a `build` job that is **independent** of the
  failing Desktop contract-corpus drift check.
- `ci.yml` `push.branches` already contains `'lane/**'`, so this branch fires
  CI without any workflow edit. Confirmed: nine runs exist for `lane/23B-03`.

So the artifact route is real and needed no rule bent. It did not complete
for one measured reason:

```
$ gh api 'repos/FerroxLabs/wayland-core/actions/runs?status=queued'  --jq .total_count
11
$ gh api 'repos/FerroxLabs/wayland-core/actions/runs?status=in_progress' --jq .total_count
0
```

**Eleven runs queued, zero in progress** — `lane/29-01`, `lane/26c`,
`lane/28-02`, `lane/red-repair`, `plan/f20-unified-audit-repair`,
`lane/28-01`, `lane/24e`, `lane/23B-04`, `lane/23B-03`, `lane/24d`,
`lane/26b`. The frontier execution itself has saturated the org's Actions
capacity. Run `30277494031` (this lane's HEAD) sat at `status: pending` with
**zero jobs started** for the remainder of the session, so no `build` job ran
and no artifact was produced. Every earlier run on this branch shows
`conclusion: cancelled` — trap #2 in the intel doc: `cancel-in-progress`
protects a *started* run, not a queued one, and this lane pushed nine times.

I did not clear the queue. Cancelling ten other lanes' runs is not this
lane's to do.

### What was explicitly NOT done to manufacture a macOS row

- **Not** built with Cargo on the Mac. Forbidden by the controlling
  instruction, and the reason a previous lane escalated rather than proceed.
- **Not** driven against `/opt/homebrew/bin/wayland-core`. That binary is
  v0.12.12 and predates this work by many commits; the driver's `--build-info`
  provenance assertion would have refused it, correctly.
- **Not** driven against a darwin artifact from some *other* branch's run.
  The driver asserts the binary's source SHA equals the commit under test, so
  such a binary would redden — and a number measured against code that is not
  this code is not evidence for this code.
- **Not** closed by grepping this file. That is the specific tautology the
  plan names by hand, and it is the reason this section says NOT ACHIEVED
  instead.

### What it would take

One `build`-job run on `lane/23B-03` at commit
`3cf304e90aaee21ed8993ee14163501ee020a81a` or later, then:

```bash
gh run download <id> -R FerroxLabs/wayland-core -n wayland-core-aarch64-apple-darwin
chmod +x wayland-core
NONCE=$(/usr/bin/openssl rand -hex 8)
bash scripts/f23-index-drive.sh --binary ./wayland-core --sha <sha> --nonce "$NONCE" \
  > .planning/phases/23B-continuous-agency/evidence/23B-03-macos-drive.log 2>&1
rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "F23_03_DRIVE=PASS platform=macos nonce=$NONCE" \
  .planning/phases/23B-continuous-agency/evidence/23B-03-macos-drive.log
```

The driver already resolves `PLATFORM=macos` from `uname -s` and needs no
change. Nothing else blocks the leg.

---

## 7. Findings

No CRITICAL or HIGH finding is open. The one CRITICAL threat in the plan's own
register — T-23B03-01, gitignored or out-of-root content persisted into the
store — is closed by measurement on both platforms that ran, with a control
marker proving the assertion was capable of failing.

| ID | Severity | Finding | Disposition |
|---|---|---|---|
| 23B-03-M1 | MEDIUM | Over the full 3,603-file workspace, `precision@1` is **0.8125**: three concept-shaped queries (`content hash invalidation`, `worktree identity`, `bm25 full text`) rank a prose-heavy planning document or another crate's doc-comment above the definition. Recall@10 is 1.0000, so nothing is lost — it is ordered wrong. This is precisely the class the OPTIONAL semantic layer addresses, and that layer is deliberately not built. Identical on Linux and Windows. | BACKLOG, non-blocking. Do **not** close by trimming the corpus. |
| 23B-03-M2 | MEDIUM | On Windows the scope fingerprint carries the verbatim extended-length prefix, slash-normalised: `gitdir=//?/C:/Users/…`. Self-consistent — both operands pass through the same function and the branch-switch comparison worked — but a fingerprint produced without `fs::canonicalize` would not compare equal to one produced with it. | BACKLOG, non-blocking. |
| 23B-03-M3 | MEDIUM | The exact-search fallback is a full scan of stored text via `instr()`: **51,601 µs** against 5,810 µs for an indexed query, ~9× slower. It is bounded by the caller's limit so it is not a DoS surface, but a caller issuing many punctuation-heavy queries pays for it. | BACKLOG, non-blocking. |
| 23B-03-L1 | LOW | `cargo hakari verify` could not be run: `cargo-hakari` is absent from `hetzner-dsm` and is not a CI step. The property it guards was checked directly — the lock diff is 2 dependency edges and 0 `[[package]]` entries. | Reported as NOT RUN, not as a pass. |

### Pre-existing, proved so, and not this plan's

`wcore-cli::child_authority_corpus::{corpus_time, corpus_token, corpus_cost,
corpus_depth}` fail on `hetzner-dsm`. They fail **identically at the untouched
base** `32e2f57d09fe4b287e513081862217dc9daa5901`, on a tree asserted by
`test ! -f crates/wcore-repomap/src/store.rs` not to contain this plan's work.
Their own assertion text names the cause and dates it: *"a production file
forwards a child-supplied budget override:
crates/wcore-agent/src/spawner.rs. This is EXPECTED from 2026-07-27."* They
belong to the F21-02 child-budget work. Reported rather than hidden, and not
counted as this plan's green.

### One defect found and fixed inside this plan's own gates

The Windows `tar` host-spec failure (§5) was found by the driver, reported by
the driver as a hard non-zero exit with no PASS marker, and fixed at the
class rather than the instance. It is recorded because it is the clearest
demonstration in this plan that the gate discipline works: forty lines of
correct measurements had already been printed above the failure, and a
`| grep` gate would have greened on them.
