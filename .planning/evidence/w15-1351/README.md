# wayland#1351 — the migration race and the silence that followed it

Host `hetzner-dsm` (Linux 6.8.0-101, 96 CPU, 251 GiB), through
`tools/remote-proof.py` slot `parallel-1`. Every run below carries a JSON
receipt with `remote_exit` and `"complete": true`.

## c1 — two openers of a migrating store

Instrument: `crates/wcore-memory/tests/issue_1351_migration_race_test.rs`,
run under `cargo test` (NOT `nextest`). The hazard is between two SQLite
CONNECTIONS, and `cargo test` puts them in one process with one barrier; the
file-lock behaviour is identical to two processes, and nextest's
process-per-test would have hidden the barrier rather than sharpened it.

RED (source `4dd7b858c`, the repairs removed, the tests kept):

    round 0: 3 of 4 concurrent openers lost the migration race
    Failures: ["migration error at v5: duplicate column name: last_latency_ms",
               "migration error at v5: duplicate column name: last_latency_ms",
               "migration error at v5: duplicate column name: last_latency_ms"]

That is the ticket's error verbatim. Each of those three is a session that
`bootstrap.rs:2231` would have dropped to `NullMemory`.

GREEN (source `66936d217`): `2 passed; 0 failed`, six rounds of four openers,
every round ending at `CURRENT_VERSION` with `procedures.last_latency_ms`
present, and `CONCURRENT_MIGRATION_RECOVERIES > 0` so the run is not vacuous.

A SECOND ARM OF THE SAME RACE was found by measurement, not by reading, and it
is why the version-keyed retry alone was not enough. With the retry in place
but no busy handler, a loser still failed:

    round 2: 1 of 4 concurrent openers ... Failures: ["memory DB: database is locked"]

Stage-tagging the open (temporary commit, discarded) put it at the journal-mode
pragma, not the ladder:

    DIAGSTAGE=journal database is locked

Switching a store into WAL takes an exclusive lock and SQLite returns
`SQLITE_BUSY` for it WITHOUT consulting the busy handler, so that call needed
its own bounded wait. `SqliteJournalMode`'s WAL arm is deliberately
byte-identical to its pre-fix behaviour and sets no busy timeout at all; the
wait is therefore installed in `apply_migrations`, scoped to the open, and
restored afterwards so no later memory write changes behaviour.

## c2 — the user is told, on a stream they actually read

Graded on a CAPTURED STREAM from the real binary, not on the sink alone.
`capture.sh` (in this directory) runs `wayland-core "say hi"` with `RUST_LOG`
UNSET (`env -u RUST_LOG`), an isolated `WAYLAND_HOME`, and `WCORE_MEMORY_DIR`
pointing at a store stamped `schema_version = 99` so `Memory::open` fails
closed. Both stdout and stderr are captured. A CONTROL arm runs the same
command against a healthy memory root.

    binary sha256                                                      arm       stderr  "memory" hits
    9a933450d0ce5db4a23a2f7a03af421e769d3c8c680f829a372490c449de1f99   RED   degraded   3,451 B      0
    (notice emission removed)                                          RED   control    3,450 B      0
    99adb2138e55185d01ea22bc2c899743df14eb1b2de5661342cde1d9bc5d45de   GREEN degraded   4,014 B      1
    (source 66936d217)                                                 GREEN control    3,450 B      0

THE RED IS THE POINT: on the unfixed binary the degraded run and the healthy
run differ by ONE BYTE of user-facing output (3,451 vs 3,450 — a path length),
and the word "memory" appears NOWHERE in either stream. A user whose long-term
memory had just been switched off saw a session indistinguishable from a
working one. The `tracing::warn!` at `bootstrap.rs:2232` was live in that
binary and reached nobody, which is exactly why a `warn!` -> `error!` bump was
not the repair.

On the green binary the same degraded run carries the notice on stderr and the
control arm still carries nothing — so the capture is a real instrument, not a
grep that matches anything.

Transcripts: `cli-{green,red}-{degraded,control}.stderr.txt`.

## What was NOT touched

`crates/wcore-agent/src/bootstrap.rs` is a `SOURCE_INPUTS` path
(`crates/wcore-protocol/src/contract/spec.rs`), so `desktop_contract_corpus`
reds. It was NOT regenerated — that is the integrator's single pass over the
merged tree. The drift message confirms the corpus is the only thing affected:

    schema_digest is UNCHANGED (sha256:8497e92e4ab2599201f95b2aa62c359ae2328429305e79a96761356483fc6e33):
    no wire schema moved. This is a source-hash rebase.
    source_inputs_digest: sha256:8ed295bf... -> sha256:8aa76bcc...

`quiescence_contract` passed. `cargo clippy -p wcore-memory -p wcore-agent
--all-targets -- -D warnings` and `cargo test -p wcore-memory` are both
`remote_exit 0`.
