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
| macOS (this Mac, arm64) | CI `build` job artifact `wayland-core-aarch64-apple-darwin`, run 30278953807 | `evidence/23B-03-macos-drive.log` | **PASS** |

---

## 1. Thresholds and the order they were chosen in

**Every threshold in this plan was chosen AFTER its measurement, and none was
widened after a failure.** No gate in this plan reddened and was then
loosened; where a measurement disappointed, the number is reported as it is.

| Gate | Threshold | Chosen | Measured (Linux) |
|---|---|---|---|
| Warm start reads **zero** files | `read == 0` exactly | **BEFORE** — it is the plan's definition of incrementality, not a tuning knob | 0 / 0 / 0 on all three platforms — and it went RED once, unprompted, on macOS (§6.1) |
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
| the plan's own Windows drive gate form | `powershell -File <missing>.ps1; exit $LASTEXITCODE` on SeanDesktop | exit **0** — it self-passes (§5.1); the guarded form exits **94** |
| the warm-start read-count gate, **unprompted** | the driver's own log was written into the indexed tree | macOS exit **1**, `read=1 extracted=1` — it caught one changed file in 3,610 (§6.1) |

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
`F23_03_DRIVE=PASS platform=windows nonce=49a9ca44ae600fe8` — the nonce the
caller generated for this run. Commit under test
`1eb2d7c255b8cdc4f2f194e60cd82ab6bbddfc68`, asserted against the binary's own
`--build-info` before any measurement.

The remote command string ends in an explicit `exit $LASTEXITCODE` and is
**never** piped into a filter; the status was captured on the next line and
asserted before the log was read for the marker. It additionally guards
`Test-Path scripts\f23-index-drive.ps1` — see §5.1 for why that guard is
load-bearing and not decoration.

The leg was driven three times in total, at `decbca2b…`, `3cf304e9…` and
`1eb2d7c2…`; the numbers below are the last run's, and every figure
reproduced across all runs within noise (cold build 1454–1576 ms, warm start
151–166 ms, p50 4892–5016 µs, precision 0.8125 on every run).

Corpus: `C:\ferrox-win` — **3,609 records, 37,868 symbols** (six more files
than the Linux checkout; the symbol count is identical, so the difference is
non-source).

### Cold build / warm start / size

| Sample | Cold (ms) | Warm (ms) | Warm reads | Warm re-extracts | Store bytes |
|---:|---:|---:|---:|---:|---:|
| 1 | 1533 | 151 | **0** | **0** | 66,707,456 |
| 2 | 1454 | 159 | **0** | **0** | 66,707,456 |
| 3 | 1510 | 166 | **0** | **0** | 66,707,456 |

**Warm : cold ratio ≈ 0.099.** Store size is within 152 KB of Linux for six
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

`p50 = 4,892 µs`, `p95 = 5,200 µs`, n = 20 — slightly **faster** than Linux
(5,810 / 6,159), on a much smaller machine.

### Retrieval quality

`precision@1 = 0.8125`, `recall@10 = 1.0000` — **identical to Linux, query
for query**, including which three queries lose top-1. Ranking is
platform-stable, which is the thing that would have been easiest to get
wrong and is worth having measured rather than assumed.

### Incremental mutations

All five PASS with `unchanged_reextracted=0`, against a scratch tree with
3,610 records:

| Mutation | Counter | Re-extracted |
|---|---|---:|
| add | `added=1` | 1 |
| edit | `changed=1` | 1 |
| delete | `deleted=1` | 0 |
| rename | `renamed=1` | **0** |
| branch switch | `added=1` | 1 |

### 5.1 The plan's own Windows gate form is self-passing, and this leg did not use it

`python3 .planning/scripts/lint-plan-gates.py .planning/phases/23B-continuous-agency/`
reports **HIGH `powershell-missing-script-exits-zero`** against
`23B-03-PLAN.md:253` — the plan's own Windows drive gate, the one this task
was told to run. **Proved on SeanDesktop rather than argued:**

```
$ ssh SeanD@seandesktop "Set-Location C:\ferrox-win; \
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\definitely_missing_script.ps1; \
    exit $LASTEXITCODE"
MISSING_SCRIPT_EXIT=0          <- the plan's gate form GREENS on an absent script

$ ssh SeanD@seandesktop "Set-Location C:\ferrox-win; \
    if (-not (Test-Path scripts\definitely_missing_script.ps1)) { exit 94 }; …"
GUARDED_EXIT=94                <- the guarded form REDDENS
```

This HIGH is **closed for the gates actually executed**, two ways:

1. The final Windows leg ran under the guarded form — `if (-not (Test-Path
   scripts\f23-index-drive.ps1)) { exit 94 }` in the same statement chain,
   ahead of the build and the driver — and that guard is proved able to
   return 94.
2. Every drive gate in this plan is closed by **two independent** checks: the
   process exit status *and* a `grep -qF` for a marker containing the nonce
   the caller generated seconds earlier. An absent script exits 0 but prints
   no marker, so the second check fails. That is why the *first* Windows run,
   which did use the unguarded form, was not vacuous.

The plan file itself is not this executor's to edit, so the HIGH remains open
against `23B-03-PLAN.md` and is reported here rather than silently satisfied.

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

**PASS.** `MACOS_DRIVE_RC=0`, and the log carries
`F23_03_DRIVE=PASS platform=macos nonce=3a2127430e0437db`.

### Binary provenance — no Cargo was run on this Mac

The binary is CI's own build artifact, downloaded and provenance-checked:

```
$ gh run download 30278953807 -R FerroxLabs/wayland-core -n wayland-core-aarch64-apple-darwin
$ file wayland-core
wayland-core: Mach-O 64-bit executable arm64
$ ./wayland-core --build-info
wayland-core 0.12.25 (source 1eb2d7c255b8cdc4f2f194e60cd82ab6bbddfc68)
```

`1eb2d7c255b8cdc4f2f194e60cd82ab6bbddfc68` is a commit on `lane/23B-03`
carrying this plan's code. The driver asserted that equality **before taking
any measurement**, and would have exited 68 on a mismatch.

Two things this route did NOT require, both worth stating because two earlier
lanes escalated this leg as impossible:

- **No Cargo on the Mac.** The controlling instruction is intact.
- **No `scripts/f23-macos-binary.sh`.** That script still does not exist;
  23B-01 did not land it. It was not needed, because
  `.github/workflows/ci.yml:484-490` already uploads
  `wayland-core-${{ matrix.target }}` for all six targets from a `build` job
  that is **independent** of the failing Desktop contract-corpus drift check,
  and `'lane/**'` is already in `push.branches`.

Run `30278953807` has `conclusion: failure` — the pre-existing contract-corpus
drift check — and its artifacts are still good. That is trap #1 in
`.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`, confirmed again here:
**filter runs by artifact, never by conclusion.**

The one real cost was queueing. For roughly two hours the repository showed
**11 runs queued, 0 in progress** — the frontier execution had saturated the
org's Actions capacity — and each push of this branch cancelled its own queued
run (trap #2: `cancel-in-progress` protects a *started* run, not a queued
one). The leg completed once pushing stopped and the queue drained.

Corpus: the lane worktree — **3,610 records, 37,868 symbols**.

### Cold build / warm start / size

| Sample | Cold (ms) | Warm (ms) | Warm reads | Warm re-extracts | Store bytes |
|---:|---:|---:|---:|---:|---:|
| 1 | 4560 | 210 | **0** | **0** | 66,703,360 |
| 2 | 6513 | 223 | **0** | **0** | 66,703,360 |
| 3 | 4428 | 150 | **0** | **0** | 66,703,360 |

**Warm : cold ratio ≈ 0.046.** Cold build is 3–4× Linux and Windows
(1.5 s), on the noisiest of the three hosts — this Mac was concurrently
hosting five lanes' worktrees and their ssh sessions. The spread across
samples (4428–6513 ms) is itself the evidence that it is contention, not the
index: the corpus, the binary and the code were identical across the three.

Store size is within 4 KB of Windows and 148 KB of Linux. The format is
platform-stable across all three.

### Query latency

`p50 = 6,662 µs`, `p95 = 13,004 µs`, n = 20. The p95 is 2× the p50 and 2.1×
Windows' p95, and the sample spread (3,603 µs to 20,619 µs) is far wider than
Linux's (5,485–7,171 µs) or Windows'. Same cause as the cold-build spread: a
contended host. Recorded as measured, not smoothed, and not used to set any
threshold.

### Retrieval quality

`precision@1 = 0.8125`, `recall@10 = 1.0000` — **identical to Linux and to
Windows, query for query**, including which three queries lose top-1. Ranking
is now measured as platform-stable across all three targets.

### Incremental mutations

All five PASS with `unchanged_reextracted=0` against a 3,614-file scratch
tree with 3,609 records: `added=1` / `changed=1` / `deleted=1` /
`renamed=1` (re-extracting **0**) / branch switch `added=1`.

### Fallback, staleness, verify and secret isolation

```
F23_03_FALLBACK_REPORTED=true
F23_03_STALENESS_REPORTED=true
F23_03_VERIFY=agrees=false exit=6
F23_03_STORE_CONTROL_OCCURRENCES=1
F23_03_STORE_NONCE_OCCURRENCES=0
```

### 6.1 The macOS leg went RED first, and the index was right

The first macOS run — `evidence/23B-03-macos-drive-selfwrite-red.log`, kept
deliberately — **failed with exit 1 and three failures**, all the same one:

```
FAIL: warm start sample 1 opened 1 files and extracted 1 —
      incrementality is a READ COUNT, and this one is not zero
F23_03_WARM=sample=1 read=1 extracted=1
```

Exactly one file, on every sample, out of 3,610.

The cause is the harness, not the product. On macOS the driver runs *locally*,
and its stdout was redirected into
`.planning/phases/23B-continuous-agency/evidence/23B-03-macos-drive.log` —
a tracked, in-scope file **inside the repository being indexed**, growing on
every line the driver printed. Between the cold build and the warm start,
exactly one file's bytes changed, and the index noticed. The Linux and Windows
legs never hit this because their logs are written by the `ssh` redirect on
the *caller's* machine, not inside the remote tree.

**Diagnosed by experiment, not by argument.** The identical driver, identical
binary, identical corpus, with only the log redirected outside the tree:

```
$ bash scripts/f23-index-drive.sh --binary … --sha … --nonce 3a2127430e0437db \
    > /tmp/f23-macos-drive-outside.log 2>&1
MACOS_DRIVE_RC=0
F23_03_WARM=sample=1 read=0 extracted=0
F23_03_WARM=sample=2 read=0 extracted=0
F23_03_WARM=sample=3 read=0 extracted=0
```

The red log is retained because it is the strongest single piece of evidence
in this document that the read-count gate works: it detected **one** changed
file among 3,610 and refused to call the run incremental. A stopwatch-based
warm-start assertion would have passed it without noticing — 152 ms, well
inside any plausible bound. The recorded macOS numbers above come from the
passing run; the failing run's numbers are not mixed in.

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
