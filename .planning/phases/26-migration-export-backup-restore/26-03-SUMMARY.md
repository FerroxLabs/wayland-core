---
phase: 26-migration-export-backup-restore
plan: "03"
status: partial
termination_state: "NOT REACHED — Tasks 1-2 complete and proven on Linux; Task 3 (Windows) partial; Task 4 (two panels) NOT RUN"
requirements: [F26-03, F26-04]
requirements_claimed: []
lane_branch: lane/26b
evidence_sha: d0de9c397e1b1a1da8f399cb1142e2ed3f5f125a
---

# Phase 26 Plan 03: Backup, Restore and Exact Rollback — Summary

Core now has `backup create / verify / restore / recover / digest`, a write-ahead operation
journal, and explicit per-backend credential remapping. **Exact rollback from an uncatchable
mid-flight kill is proven live on Linux against the real binary, over a target that carried
state** — with a negative control proving the mid-flight check can fire and a positive
control proving the uncatchability measurement is not vacuous.

**The plan is NOT complete and F26-03/F26-04 are NOT claimed.** Tasks 1 and 2 are done and
evidenced. Task 3 (Windows) is partial. Task 4 (two four-way panels) was **not run**, and the
plan marks its requirements complete only if every Task 4 gate passes.

All evidence below is at `d0de9c39` unless stated otherwise.

## Verdict against the plan's own criteria

| Criterion | Outcome |
|---|---|
| Subcommands exist, proven by the REAL binary's help | YES (Linux) |
| Archive embeds a manifest; refuses overwrite; refuses output inside source tree | YES — 3 separate tests |
| Verification fails on tampered payload / traversal path / missing declared payload | YES — three separate, distinguishable rejections |
| Restore verifies before writing; refuses an occupied target | YES |
| Secret sources explicitly remapped across all four backends | YES — 4/4 captured |
| Write-ahead journal, durable intent before first mutation, via `atomic_write` | YES |
| Per-operation AND per-process scoping, dead-pid rule | YES — mirrors `crash_sentinel` (#181) |
| Recovery idempotent; leaves live-owner records alone | YES |
| Exact rollback from an uncatchable mid-flight kill — **Linux** | YES |
| …the same on **real Windows** | **PARTIAL** — exact rollback PROVEN on Windows against a real long-path failure; the uncatchable-kill leg did NOT run. See "Windows". |
| Windows-only cases: long/deep restored paths | **FAILS — HIGH finding F26-03-D** |
| Windows-only case: target held open by another handle | NOT RUN |
| Partial-failure path rolls back exactly | YES |
| Proof scripts demonstrated able to go RED | YES (Linux) |
| Mid-flight check demonstrated able to FIRE | YES (Linux, exit 9) |
| Kill uncatchability MEASURED | YES (Linux) |
| Two four-way cross-audited panels (Task 4) | **NO — not run** |

## What landed

`crates/wcore-cli/src/backup/{mod,archive,restore,journal,remap}.rs`; one additive block each
in `lib.rs` and `main.rs` (the shared-file fence); and four committed proof scripts —
`portability-interrupt-proof.sh`, `portability-interrupt-proof.ps1`,
`portability-roundtrip-proof.sh`, `portability-remap-capture.sh`.

**No new dependency.** `tar`, `flate2`, `sha2`, `serde`, `serde_json`, `toml`, `tempfile`,
`chrono` were already `wcore-cli` dependencies; `Cargo.toml` and `Cargo.lock` are untouched.

Reuses rather than reinvents: `profile::is_secret_entry`, `atomic_io::atomic_write`,
`portability::tree_digest` (26-01's digest), `cron::process_is_alive`, and the real
`CredentialsBackend` enum.

## Gate results — real numbers

- `cargo fmt --all -- --check`: **clean** (after fixing a violation that was red at base).
- `cargo clippy --locked -p wcore-cli --all-targets -- -D warnings`: **clean**.
- backup-filtered: **38 run, 38 passed** — non-vacuous, a non-zero count executed.
- `cargo nextest run --locked -p wcore-cli --no-fail-fast`: **2118 run, 2114 passed, 4 failed,
  9 skipped** (measured at `1f5f9e38`; later commits add only tests and scripts).
  The 4 are `child_authority_corpus::{corpus_time,corpus_token,corpus_cost,corpus_depth}`.
  **Proven pre-existing, not asserted:** the same 4 fail identically at base `9bb3f079` in a
  separate worktree containing none of this work (4 run / 0 passed / 4 failed). Their own
  message names `crates/wcore-agent/src/spawner.rs` — a file this work never touches — and
  says "This is EXPECTED from 2026-07-27". **My delta on `wcore-cli` is 0 new failures.**
  Not fixed: another lane's area, out of scope.

### The real binary's help (Linux)

```
Commands:
  create   Create a verifiable archive of a Wayland home
  verify   Verify an archive's manifest, payload digests and entry paths
  restore  Restore an archive into a home directory
  recover  Roll back any operation whose owning process died mid-flight
  digest   Print the tree digest of a home, and the algorithm used
```

## Live evidence — Linux interruption and exact rollback

Real release binary, `SIGKILL`, over a target that **carried state**: a file the archive
overwrites, a top-level file it does not contain, and a whole directory that must come back.

```
INTERRUPT-PLATFORM: linux
KILL-MECHANISM: SIGKILL CATCHABLE: no
KILL-HANDLER-PROBE: installed=yes fired=no
FIXTURE-PAYLOADS: 120
MIDFLIGHT-JOURNAL-OPEN: yes
MIDFLIGHT-TARGET-INTERMEDIATE: yes
MIDFLIGHT-TIMING: op_expected_ms=3199 kill_at_ms=900 completed_before_kill=no
DIGEST-ALGO: sha256/wcore-portability-tree-v1 path-norm=slash-relative content=raw-bytes
DIGEST-PRE:  4069e26f5d5527be57482afe86f6c422b22d19a5ee9574cf44fb087b89542a40
DIGEST-POST: 4069e26f5d5527be57482afe86f6c422b22d19a5ee9574cf44fb087b89542a40
DIGEST-EQUAL: yes
PROOF-OK: exact rollback from an uncatchable mid-flight kill, over a target that carried state
```

**Three controls, because each alone is consistent with a vacuous pass:**

1. **Self-red** — nonexistent binary → exit **1**.
2. **Negative control** (`--undersized`): `op_expected_ms=60` vs `kill_at_ms=900`, so the
   operation finished first; the script **detected the late kill and exited 9**
   (`NEGATIVE-CONTROL: late-kill-detected`). Without this, `DIGEST-EQUAL: yes` would be
   equally consistent with a mid-flight check that never runs.
3. **Handler positive control** (SIGTERM): `fired=yes`. So `fired=no` in the real run is a
   **measurement of uncatchability**, not the absence of a probe that was never installed.

## Live evidence — the round trip (the deliverable)

Adjudicated by `diff -r` against the real binary, not by a digest alone.

- **Leg A, full fidelity (`--include-secrets`):** 5 payloads, `diff -r` **empty**, both canary
  secret values present in the restored tree. Lossless, and the secrets provably came back.
- **Leg B, redacted (default):** 3 payloads. The default restore **refused**; the refusal left
  the target **unwritten**. With `--accept-missing-secrets`, `diff -r` reports **exactly two**
  differences — `credentials.toml` and `oauth` — and nothing else. **Zero** canary values
  appear in the restored tree, while the identical search **does** find them in the source
  (positive control), so the absence measures redaction rather than a broken grep.

Additional state-carrying restores, all over an occupied target, all asserted by digest:
**restore over a diverged profile** (extra dir + edited file → archive's tree exactly);
**restore from an older-schema archive** over an existing profile (older manifest *shape*,
written as raw JSON, not a cleared newer struct); **restore from a partially-written backup**
(truncated mid-file) leaves a live target **byte-identical with no journal ever opened**; and
**partial failure part-way through the write loop** rolls back to the exact prior tree.

### What a redacted export cannot round-trip — stated plainly

**A redacted archive cannot restore the secret values it never carried, and no round trip can
recover them.** By default `backup create` omits exactly the entries
`wcore_config::profile::is_secret_entry` classifies — `credentials*` files and `oauth/` — and
records their names in the manifest as `absent_secrets`.

Any claim that a redacted round trip is lossless would be **false**. What is true, and what
the suite asserts, is narrower and checkable: the difference is *exactly* the recorded names
and nothing else; the omission is *declared* rather than silent; and the restore *refuses* by
default rather than emitting a config pointing at credentials that are not there.
`--include-secrets` is the mode that round-trips losslessly, and it is opt-in precisely
because such an archive carries live credentials.

## Live evidence — per-backend remap (all four)

| Backend | exit | disposition | target written | names backend/count/action | source abs path survives |
|---|---|---|---|---|---|
| `auto` | 1 | refused | **no** | yes / yes / yes | no |
| `plaintext` | 1 | refused | **no** | yes / yes / yes | no |
| `keyring` | 1 | refused | **no** | yes / yes / yes | no |
| `encrypted-file` | 1 | refused | **no** | yes / yes / yes | no |

`target written` is **measured** by digesting the target before and after, not read off the
message — that is how a warn-and-continue would be caught. **No refusal wrote its target.**

The absolute-path result is **non-vacuous**, proven separately: a source config naming
`/machine-only/credentials.enc` and `/machine-only/credentials.kdf.json` (**2** absolute paths)
restores to a config naming `<target>/credentials.enc` and `<target>/credentials.kdf.json`
(**0** surviving). The rewrite happened; the search did not merely find nothing.

Verbatim operator message (keyring):

```
credential remap: backend `keyring` — 2 credential source(s) will NOT be present after restore.
absent: credentials.toml, keyring-stored secrets (OS keychain)
action: re-add these credentials on this machine (`wayland-core auth add <provider>`) after the
restore, or re-run with --accept-missing-secrets to proceed knowing the restored install starts
without them.
```

## Windows — PARTIAL, and it found a real defect

Run on `SeanD@seandesktop`, release binary built on the box at `d0de9c39`, with the checkout
SHA captured to its own file and compared before AND after the run (both
`d0de9c39...` — no interference).

**What WAS proven on Windows:**

| Measurement | Result |
|---|---|
| `backup create` on a deep tree | exit 0 — the archive is produced |
| `backup restore`, shallow tree, into an empty target | exit 0 |
| Shallow round trip, source vs restored digest | **identical** (`4ca35eec…`) |
| `backup restore` of that deep archive | **exit 1 — `io error while write restored payload: The system cannot find the path specified. (os error 3)`** |
| Rollback after that failure, over a target holding a LIVE profile | **exact** — `PRE_DIGEST == POST_DIGEST` (`2c66533c…`), `config.toml` still `LIVE-PROFILE`, and `legacy/keep.txt` (a file the archive does not contain) still present |

So the **exact-rollback contract HOLDS on Windows**, and it was proven against a *real
Windows-specific failure* rather than an injected one — which is stronger evidence for that
particular property than the signal path gives.

### FINDING F26-03-D (HIGH) — `backup create` produces archives that `backup restore` cannot restore on Windows

A payload whose reconstructed target path exceeds Windows' `MAX_PATH` (260) fails the restore
with `os error 3`, while `create` accepted the same tree without complaint. The archive is
therefore **silently unrestorable on the platform it was made on**. This is exactly the case
26-03 predicted ("a backup that cannot restore its own deepest file is a backup that does not
work") and the reason it insisted on a Windows leg.

Bounded, not general: a shallow restore round-trips byte-identically on the same binary, so
this is long-path handling, not restore.

**Remediation (not applied here):** the restore write path needs Windows extended-length
(`\\?\`) path handling in `restore.rs` / `atomic_io`, and `create` should refuse — or at least
warn — when a payload's reconstructed length would exceed the target platform's limit, rather
than producing an archive that cannot be restored. Not fixed in this plan: each Windows
iteration on that shared box costs 10-20 minutes and I ran out of budget before I could fix
and re-prove it. **I am reporting it red rather than narrowing the fixture until it passed.**

### What was NOT achieved on Windows

**The uncatchable-kill (TerminateProcess) mid-flight interruption leg did not run.** The proof
script aborts in its own timing run, because that run restores the deep-path fixture and hits
F26-03-D above. So on Windows the SIGKILL-equivalent leg, its negative control and its handler
control have **no result** — not a pass, not a fail. Removing the deep path to get the leg
green would have been narrowing the test to reach a green, so I did not.

Two harness defects were fixed along the way and are worth recording because both are
self-passing shapes:

- The script named a parameter `$home`. `$HOME` is a **read-only automatic variable** in
  PowerShell, so it could never bind; every call died at run time with "Cannot overwrite
  variable home". A syntax check passes this — only running it on the box finds it.
- `echo DONE=1>> file` in `cmd` parses `1>>` as a **file-descriptor redirect**, so every exit
  marker I wrote recorded an empty value. Fixed by putting the redirect first
  (`>>file echo DONE=1`).

A dedicated Windows worktree (`C:\ferrox-win-f26`) was attempted first, to avoid the shared
checkout that another lane moved out from under this plan mid-run (observed: HEAD went to
`f0778ba1` between two of my steps — the plan's SHA assertion caught it rather than certifying
the wrong commit). The worktree was abandoned because a cold build there crashed `rustc` twice
with `STATUS_ACCESS_VIOLATION` on `cranelift-frontend` (at default and at `-j 4`), while the
shared checkout builds because its `target/` is warm.

## Findings

**F26-03-A (MEDIUM, BACKLOG) — the manifest has the same untyped-string-channel shape that
26-01 closed in `DiscoveredItem.details`.** `CredentialCapture.external_paths` is a
`BTreeMap<String, String>` that copies `cipher_path` / `key_params_path` **verbatim** out of
the source `config.toml` into the embedded manifest — a document that by design travels to
another machine and is the part an operator is most likely to read, log or share. The values
are `PathBuf`s and so are user-controlled. No secret *value* is carried today, and the archive
payload is separately gated by `--include-secrets`, which is why this is MEDIUM rather than
HIGH. But the *channel* is the one 26-01 found: an untyped string crossing a trust boundary.
**Remediation:** the remap needs only the file NAME (to rebuild a target-side path) and the
inside/outside-home boolean; the full source path is operator context and should be a typed,
validated path field or dropped. Not changed here because the format is what the platform
evidence was produced against; changing it mid-run would have invalidated the measurements
without re-running them.

**F26-03-B (MEDIUM, BACKLOG) — `cargo fmt --all -- --check` was red at base for every lane.**
`crates/wcore-agent/examples/p22_goal_live.rs` landed unformatted in `91979ec8` (lane 22).
Fixed here in its own droppable commit `674b5b5f`; see deviations.

**F26-03-C (MEDIUM, BACKLOG) — `powershell -NoProfile -File <missing.ps1>; exit $LASTEXITCODE`
returns 0.** A Windows gate written in that shape reports success when the script it names does
not exist. This is a live self-passing-gate shape on the shared box and it bit this plan (see
Windows). Every Windows gate must assert the script exists first — my driver now does
(`if not exist ... exit /b 44`).

## Deviations, each with its reason

1. **`p22_goal_live.rs` reformatted** (commit `674b5b5f`, droppable) — the fmt gate was red at
   base and could not distinguish my work from lane 22's drift. **Flagged loudly** per the brief.
2. **`backup digest` subcommand added.** The plan requires both platforms' `DIGEST-ALGO` to be
   byte-equal before any digest comparison is believed. Exposing the product's own digest makes
   that a *property* (same constant, same code path) rather than a copied string, and stops each
   proof script from reimplementing the digest and comparing its own arithmetic.
3. **`--pace-ms` hidden test seam on `restore`.** The plan asks the fixture to be sized so the
   operation "genuinely takes time on that hardware". Pacing controls that directly and
   portably. It changes only *when* bytes are written, never which bytes or in what order, and
   **no assertion is weakened** — the negative control still fires.
4. **`--replace` and the state-carrying target.** Refusing an occupied target is the default and
   holds. But a restore that can only write into an empty directory has a trivial rollback, so
   `--replace` (which journals the entire prior tree first) is the path the proof exercises.
5. **Two extra scripts** (`portability-roundtrip-proof.sh`, and the `--handler-control` mode)
   beyond the plan's declared files — the round trip is the phase's headline deliverable and
   deserved committed evidence; `fired=no` alone proves nothing without its positive control.
6. **A dedicated Windows worktree was attempted and abandoned** — see Windows.

## Defects the gates actually caught (evidence they can fail)

1. **`toml::Value` vs `toml::Table`.** `Value: FromStr` parses a TOML *value*, so a document
   beginning `[storage.credentials]` was read as an **array** and rejected — and `read_backend`
   discarded that with `.ok()`, so **every** home reported "no credentials backend declared".
   An archive of a keyring home would have claimed a complete capture of secrets it never
   located. Caught by 3 unit tests. A *declared but unparseable* backend is now an error.
2. **The traversal fixture could not be built** — the `tar` crate refuses to write a `..` name,
   so the test failed in its own setup. Replaced with a hand-built ustar: the difference between
   testing the verifier and testing our own writer.
3. **A self-passing remote gate, mine.** My first hetzner build piped `cargo build` into `tail`
   *inside* the ssh, so `set -e` saw `tail`'s status. That gate could not have gone red — trap 2
   from the plan, which I walked straight into. Re-run unfiltered.
4. **The Windows proof script did not parse, and failed in a way that reads as a pass.** It
   exited 1 — which a careless reading scores as a passing self-red check — but from a **parse
   error**, not the missing binary it was handed. Four UTF-8 em-dashes with no BOM: PowerShell
   5.1 reads a BOM-less script as ANSI, so `E2 80 94` decodes ending in `0x94` = U+201D, and
   **PowerShell accepts smart quotes as string delimiters**, so the em-dash closed a string
   mid-line. The file is now pure ASCII.
5. Two `clippy::collapsible_if` sites, and two `non_snake_case` test names (renamed, not
   `#[allow]`ed).

## Not done — stated plainly

- **Task 4 (two four-way panels) was NOT run.** No `panel/26-03-interruption-nonvacuity/` and
  no `panel/26-03-remap-operability/` exist. The *measurements* both panels were to judge are
  captured above; the panels, their `DECISION.md` records and their dissent are absent.
  **F26-03 and F26-04 are not claimed.**
- **The Windows uncatchable-kill leg did not run** (blocked by F26-03-D), together with its
  negative control, its handler control and the open-handle case. Those are *measurements that
  did not run* — they must not be read as either a pass or a fail. What IS known on Windows is
  in the Windows section: a byte-identical shallow round trip, a HIGH long-path defect, and
  exact rollback over a live profile under that real failure.
- **Plans 26-02 and 26-04 were not started.**
- Archive *creation* is not separately interrupted; its publication is atomic by construction
  (`atomic_write`), which is an argument, not a measurement.
- Every fixture is synthetic and canary-seeded. This plan's rules forbid pointing backup or
  restore at a real home on any host, so — unlike 26-01, which read Sean's real peer installs —
  no real profile was archived. That is a deliberate scope limit, not an oversight, and it
  means the round-trip evidence is against a realistic synthetic home rather than a real one.
