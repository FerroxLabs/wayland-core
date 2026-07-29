---
lane: restore-rollback-sqlite
defect: BL-F26-SC3-O1-ROLLBACK
severity_at_open: HIGH (silent rollback corruption — same shape as F26-SC3-O1)
status: FIXED and live-proven
branch: lane/restore-rollback-sqlite
base: 2c8b6d1d097e5bf7e2785dae349b5b6a31ea5160 (gh/plan/f20-unified-audit-repair)
touched_journal_rs: yes — unavoidable; all four journal-dependent interruption proofs re-run
findings: "BL-F26-SC3-O1-ROLLBACK (fixed) — an interrupted `restore --replace` rolls the user back to a SQLite database that was never consistent, 7 of 8 runs, while restore and recover both exit 0; BL-F26-SC3-O1-ROLLBACK-I1 (fixed in-lane) — the proof harness summed signed row differences and could cancel a real loss against the impossible surplus a corrupt btree reports"
---

# BL-F26-SC3-O1-ROLLBACK — the undo store over a live SQLite home

## Verdict

**Fixed, and proven by a demonstrated corruption rather than an assertion.**
The defect reproduced **7 of 8** runs through the real binary; the fix passes
**4/4 under strictly heavier concurrency**, plus both controls; and every
interruption proof that depends on `journal.rs` was re-run and reported.

I also found the defect in a **second journal mode** nobody had asked about —
the one a network filesystem selects — and it is fixed there too.

## The defect

`restore --replace` captures the prior home into the journal's undo store, and
that capture was `std::fs::copy` per file walked with `read_dir`
(`journal.rs::copy_inner`). A WAL trio — `memory.db`, `-wal`, `-shm` — was
therefore read at three different instants. Interrupt the restore, and
`backup recover` copies that capture straight back into the user's home.

Both the archive-side lane and this one land on the same sentence: a SQLite
database is not a file, and code that treats it as one loses data silently.

## The known-negative: measured, not argued

`scripts/sqlite-restore-rollback-proof.py` drives the real binary while three
stock-Python `sqlite3` writers commit into a 307 MiB `memory.db`, SIGKILLs the
restore, and runs `backup recover`.

Binaries (`sha256sum`, unproxied, `hetzner-dsm`):
- BASE `7cf8d45d8e4dad5c94cf5afd73dd6b3fea1083d32516b14a0d34af9c4bbb6972` @ `2c8b6d1d`
- FIX  `a242cc99ddc9bdaeb812ba87926706a3f407038661ae8bfdae5c150eff35ee4f` @ `79087cd4`

| binary | arm | rolled-back sidecars | `integrity_check` | rows lost | verdict |
|---|---|---|---|---|---|
| base | concurrent c1 | `-shm`, `-wal` | **corrupt**, 101 lines | 0 | FAIL |
| base | concurrent c2 | `-shm`, `-wal` | **corrupt**, 100 | 0 | FAIL |
| base | concurrent c3 | `-shm`, `-wal` | **corrupt**, 101 | *(+107 impossible surplus)* | FAIL |
| base | concurrent c4 | `-shm`, `-wal` | ok | 0 | **PASS** |
| base | concurrent r1 | `-shm`, `-wal` | **corrupt**, 100 | 0 | FAIL |
| base | concurrent r2 | `-shm`, `-wal` | **corrupt**, 101 | **16** | FAIL |
| base | concurrent r3 | `-shm`, `-wal` | **corrupt**, 1 | 0 | FAIL |
| base | concurrent r4 | `-shm`, `-wal` | **corrupt**, 1 | 0 | FAIL |
| base | **sequenced** (control) | none | ok | 0 | **PASS** (x2) |
| base | **truncate** (network-fs mode) | `memory.db-journal` | **corrupt**, 100 | **56** | FAIL |
| **fix** | concurrent f1-f4 | **none** | ok | 0 | **PASS 4/4** |
| **fix** | sequenced (control) | none | ok | 0 | **PASS** |
| **fix** | **truncate** | **none** | ok | 0 | **PASS** |

`backup restore` exited 0. `backup recover` exited 0 and correctly reported
`recovered_operations: 1`. Nothing warned.

Two corruption signatures at base: `*** in database main ***` at SQLite's
100-101 line reporting cap, and
`wrong # of entries in index sqlite_autoindex_rows_committed_1`.

### `base-c4` PASSED, and I am reporting it

One base run came back clean. The defect is **probabilistic**, which is exactly
why a single green run proves nothing here and why the brief demands a
demonstrated red. Recording only the seven failures would have been a nicer
table and a worse result.

### The control is the load-bearing half

`--arm sequenced` runs the SAME writers over the SAME rows and waits for every
one to EXIT before the restore starts: **65,914 rows demanded, all present,
`integrity_check=ok`, no sidecars**. Concurrency is the only variable between
the arms, and the control demanded a non-trivial row set, so the pass is not
vacuous.

### The fixed arm is the HARDER experiment, not an easier one

A fix that merely made the capture faster could dodge the window rather than
close it. It went the other way:

| | capture window | commits by each writer across it |
|---|---|---|
| base | 0.96 - 1.06 s | ~ +2,000 |
| fix  | **2.21 - 2.77 s** | **~ +3,700** |

The fixed arm sat in the window **more than twice as long** under **~85% more
concurrent commits**, and passed 4/4.

### Anti-vacuity guards, all of which abort rather than pass

1. every writer wrote a START marker, written only after its **first commit
   succeeded** — process launch is not enough (§6a-i);
2. the journal mode is read back from each writer's own marker, never inferred
   from the PRAGMA we sent (§3b-ii) — this is what makes the `truncate` arm
   trustworthy;
3. **`preserved: true` was observed in the product's own journal record** before
   the kill — a blind timer can land before the capture finishes, leaving
   `preserved: false`, where recovery correctly does nothing and the run would
   report a clean home having tested no rollback at all;
4. every writer's committed count increased **across the capture window**;
5. `backup recover` must report `recovered_operations: 1` — if it reports 0
   there was no rollback to test and the run is void;
6. a pass demanding zero rows downgrades to `ABORTED-VACUOUS`.

The kill is reaped before recovery, deliberately: `journal::recover` acts only
on records whose owner is DEAD, and `kill(pid, 0)` on an unreaped **zombie**
still reports alive — an unreaped child would make recovery skip the record and
the home would come back clean having rolled back nothing.

## An instrument defect of my own, repaired in-lane (§6b-ii)

Run `base-c3` reported `MISSING-COMMITTED-ROWS: -107`. Under
`PRIMARY KEY (wid, n)` with `n = 1..need`, `COUNT(*) WHERE n <= need` **cannot**
exceed `need` — the corrupt btree inflated the count. My accounting summed
SIGNED differences, so such a surplus can cancel a genuine loss from another
writer and report a clean zero.

Repaired, not written up: `account_rows()` clamps per writer and reports
`IMPOSSIBLE-SURPLUS-ROWS` as its own failing finding.
`scripts/sqlite-restore-rollback-selftest.py` carries the three assertions:

```
A1 intact: missing=0 surplus=0 detail={'w0': 0, 'w1': 0, 'w2': 0}
A2 lost-50: missing=50 surplus=0 detail={'w0': 50, 'w1': 0, 'w2': 0}
A3 cancelling: OLD signed-sum=0 | NEW missing=107 surplus=107 detail={'w0': 107, 'w1': 0, 'w2': 0}
A3 verdicts on the same data: OLD=PASS NEW=FAIL
ASSERTIONS: 3  FAILURES: 0
VERDICT: PASS
```

A3 is the load-bearing one: the old matcher returns a clean **PASS** on data
that lost 107 committed rows.

## Did I touch `journal.rs`? Yes — and here is the re-proof

**Unavoidable.** The raw `std::fs::copy` *is* `copy_inner`, inside `journal.rs`.
Intercepting upstream in `restore.rs` would have left the defective primitive in
place for every other caller of `preserve_target` — the silent-degradation shape
the brief forbids. So the constraint the archive lane deferred had to be paid,
not dodged.

**Which proofs depend on it** (measured before changing anything, with a
liveness control in the same sweep — `grep -c "config.toml"` returned 4/2/1/0
over the same file list, so it discriminates):

| proof | depends on `journal.rs`? | result at FIX |
|---|---|---|
| `portability-interrupt-proof.sh` | yes (`backup recover` L190) | `DIGEST-EQUAL: yes` / **PROOF-OK** |
| `portability-backup-rollback-sweep.sh` | yes (`backup recover` L205) | **PROOF: PASS** — rollback-arm byte-diff **0**, known-negative arm damaged **9/9**, unverifiable archives **0** |
| `portability-migrate-rollback-proof.sh --peer openclaw` | yes | **PROOF: PASS** — byte-diff **0**, negative arm **6** |
| `portability-migrate-rollback-proof.sh --peer hermes` | yes | **PASS on re-run** — see below |
| `portability-migrate-interrupt-proof.sh` | **no** journal refs | **PROOF: PASS peer=hermes trials=9 mid=9 recovered=9** |

### The one that failed, and why it is not a regression

The first hermes run reported
`PROOF: FAIL — only 1 of 9 kills landed mid-apply; the window was not exercised`.
That is the harness's **own anti-vacuity guard** firing on kill TIMING, not a
byte-identity failure — `ROLLBACK-ARM-BYTE-DIFF-UNEXPLAINED: 0` in that very
run.

I did not report it either way until a control said which it was. Paired A/B,
same host, same hour, base binary vs fix binary, three runs each:

```
base   run1/2/3: ROLLBACK-ARM-BYTE-DIFF-UNEXPLAINED: 0  NOROLLBACK-ARM-MID-DAMAGED: 6  PROOF: PASS
fix    run1/2/3: ROLLBACK-ARM-BYTE-DIFF-UNEXPLAINED: 0  NOROLLBACK-ARM-MID-DAMAGED: 6  PROOF: PASS
```

**6/6 PASS, base and fix indistinguishable.** Host load was ~8.6 during the
failing run and ~3.8 during the A/B; kill timing is stochastic and the prior
lane recorded the same mid/pre/post split moving between runs.

### Why the change is a NO-OP for every existing proof corpus

Searched all three journal-dependent proofs plus both corpus generators for a
SQLite database. Exactly one hit:

```
scripts/portability-migrate-rollback-proof.sh:173:printf 'PRIOR-USER-MEMORY-DB\n' > "$TEMPLATE/memory.db"
```

That file is **named** `memory.db` and is a 22-byte text stub with no
`SQLite format 3\0` header. Detection is by **header magic, never filename**, so
it is byte-copied exactly as before —
`a_file_merely_named_like_a_database_is_still_byte_identical` pins that, and it
passes in both the fixed and the reverted arm, which is the point.

## The design

`copy_inner` gains an explicit **direction**, because the two are not symmetric:

* `SqliteMode::Capture` reads the user's LIVE home → each database goes through
  `wcore_config::sqlite_snapshot::snapshot_database` (reused **as-is**; that file
  is unmodified) and its derived sidecars are dropped;
* `SqliteMode::Verbatim` reads the undo store, which is quiescent by
  construction → byte copy. Re-snapshotting there would be pointless work and
  would open a read-write connection against a store whose whole job is to be
  handed back unchanged.

Databases and sidecars are decided for the **whole directory before anything is
read**, for the reason `archive::SqliteCapturePlan` documents: a `-wal` is only a
sidecar if the file it names is genuinely a database.

**Failure refuses the operation.** A capture that cannot be verified returns
`BackupError::SqliteCapture`; it never falls back to `fs::copy`. That fallback
would silently reinstate the defect behind an undo store now claiming to hold a
consistent home — and would do it under a passing test.

### One hazard I closed that was not in the brief

`restore_scope()` removes only scope entries, so a preserved database restored
as a folded capture could have had a **stale `-wal` left beside it** — SQLite
would replay a WAL over a file it does not belong to. Unreachable with today's
`MIGRATE_SCOPE` (`quarantine`, `migrate-imported`, `skills`, `config.toml` — no
database), so it is guarded rather than urgent. The cost is three `remove_file`
calls; the alternative is a silent corruption the day someone adds a database to
that list.

### What the fix does NOT promise

A rolled-back SQLite database is a consistent **equivalent database, not
byte-identical bytes** — folding a WAL is the point, so it cannot be. SC3's
clause is "restore exact pre-operation state", and this is a genuine, named
narrowing of it.

It is worth being precise about what was traded: for a live database that clause
was **already unsatisfiable**. The bytes restored at base were a torn mixture —
not the pre-operation state either, merely a corrupt approximation of it. So the
trade is an unachievable byte-exactness for an achievable consistency. For every
tree containing no live database, byte-exactness is unchanged, and the existing
proofs demonstrate that rather than assert it.

## Gates (unproxied `cargo` over ssh; `hetzner-dsm`)

```
cargo test -p wcore-cli --lib -- backup:: migrate::
    102 passed; 0 failed; 0 ignored; 0 measured; 1776 filtered out   [base 98; +4 mine]
cargo test -p wcore-cli --lib -- backup::journal::
    16 passed; 0 failed; 0 ignored; 0 measured; 1862 filtered out
cargo test -p wcore-config --features sqlite --lib
    575 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
cargo test -p wcore-cli --test backup_sqlite_capture
    3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
cargo clippy -p wcore-cli --all-targets      clean (only pre-existing imap-proto future-incompat)
cargo fmt --all -- --check                   rc=0, zero output (Mac)
harness self-test                            3 assertions, PASS
```

The `sqlite`-feature trap the archive lane named was watched for: my code is in
`wcore-cli`, which already declares
`wcore-config = { features = ["sqlite"] }` unconditionally, so no `#[cfg]` gate
could silently drop it. The count moving 98 -> 102 is what proves all four new
tests actually compiled and ran, and `0 ignored; 0 filtered out` is read back
from an unproxied cargo.

### One failure in the full `wcore-cli` suite, pre-existing

`migrate_hermes::import_is_idempotent_without_overwrite` — **F26-SC3-O2**,
already open. The two sides of the assertion differ **only** in whether
`[profiles.alpha]` precedes `[profiles.beta]`; `ConfigFile.profiles` is a
`HashMap` and Rust seeds it per process.

Measured rather than argued, same host, same hour:

```
HEAD 05c08140 : passed=3 failed=7 of 10
base 2c8b6d1d : passed=5 failed=5 of 10
```

Both flake; base flakes at 50%. A deterministic break from my change could not
pass 3 times at my own HEAD. Not weakened, not ignored, not re-gated.

## Does `restore --replace` now round-trip a LIVE database?

**Yes — measured, not inferred.** A home whose `memory.db` is being written by
three processes, with the restore SIGKILLed mid-flight and rolled back by
`backup recover`, comes back with `integrity_check=ok`, **no sidecars**, and
every row committed before the restore was launched: 4/4 concurrent, plus the
sequenced control and the truncate arm.

## Network filesystems and Windows

**Network filesystem — partially, and I will not overclaim.** I did **not** run
on an actual NFS/CIFS mount. NFS is installable on `hetzner-dsm`, but starting a
system service on a box shared by five lanes was not a cost I was willing to
impose for this.

What I **did** measure is the journal mode that a network filesystem forces:
`wcore_config::sqlite_journal` selects **TRUNCATE** there, because WAL's
shared-memory wal-index is unavailable. That arm turned out to be **affected at
base and fixed by this change** —

```
base:  ROLLED-BACK-SIDECARS: ['memory.db-journal']   integrity: corrupt, 100 lines   rows lost: 56   FAIL
fix:   ROLLED-BACK-SIDECARS: []                      integrity: ok                   rows lost: 0    PASS
```

so the `-journal` sidecar branch is now exercised rather than merely assumed
benign — the archive lane's "expected benign; unmeasured" was, on this side of
the operation, **not benign**. What remains unmeasured is the filesystem itself:
locking semantics, `rename` atomicity and partial writes over the wire are not
covered by this.

**Windows — not exercised. I did not measure it, and I am not going to imply
otherwise.** The capture is platform-neutral Rust, but that is an expectation.
One concrete Windows-specific risk I can name from reading, and which a future
lane should check: the new sidecar removal in `restore_scope` calls
`remove_file` on files in a live home, and Windows refuses to delete a file
another process holds open — so a running Wayland holding `memory.db-wal` would
turn that into a rollback error. It is unreachable today (no database in
`MIGRATE_SCOPE`), which is why I am naming it rather than pre-emptively
rewriting it.

## Files changed

```
crates/wcore-cli/src/backup/journal.rs          SqliteMode, sqlite_plan, capture/verbatim split,
                                                scoped capture + stale-sidecar removal, 4 new tests
scripts/sqlite-restore-rollback-proof.py        NEW — the rollback-side proof
scripts/sqlite-restore-rollback-selftest.py     NEW — its three-assertion self-test
scripts/sqlite-backup-writer.py                 +journal-mode argument (defaults to wal; every
                                                existing caller unaffected)
evidence/26-sc3-o1-rollback/                    NOTES, KNOWN-NEGATIVE.md, proofs/ (21 logs)
```

`crates/wcore-config/src/sqlite_snapshot.rs` **not modified** — reused exactly as
the archive lane built it. `sqlite_journal.rs` **not modified**. Verified against
the merge-base `2c8b6d1d`, with a liveness control:

```
KNOWN-POSITIVE (a file I DID change):  1
FENCE (lib.rs / main.rs / .github):    0
sqlite_snapshot.rs:                    0
sqlite_journal.rs:                     0
```

## For the orchestrator to serialize

- `crates/wcore-cli/src/backup/journal.rs` — `copy_inner` gains a fourth
  parameter and is restructured collect-then-copy. The
  `26-sc3-rollback` lane owns this file; a lane touching `copy_inner`,
  `preserve_scope` or `restore_scope` will conflict non-trivially.
- **Neither shared-fence file touched.** No contract change, no PR, no merge, no
  tag, no issue closed.
- `scripts/sqlite-backup-writer.py` gained an OPTIONAL 5th argument. The archive
  lane's `sqlite-backup-consistency-proof.py` passes 4 and still gets WAL.
