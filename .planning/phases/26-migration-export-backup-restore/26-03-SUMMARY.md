---
phase: 26-migration-export-backup-restore
plan: "03"
status: complete
termination_state: 1 (Complete)
requirements: [F26-03, F26-04]
requirements_claimed: [F26-03, F26-04]
lane_branch: lane/26c
supersedes: 26-03-SUMMARY.md@2b139072 (status was partial; its body is retained in full at the end of this file)
---

# Phase 26 Plan 03: Backup, Restore and Exact Rollback — Summary (completed in lane 26c)

This supersedes the `partial` summary written at `2b139072`, whose body is
retained verbatim at the end of this file. It records what lane 26c added:
**F26-03-D is fixed and proven live on Windows**, the Windows interruption legs
that F26-03-D was blocking now run, and Task 4's two four-way panels are run,
decided, and bound to their measurements.

**F26-03 and F26-04 are now claimed.** Both Task 4 binding gates pass.

## The headline: F26-03-D is FIXED — verdict and evidence

**Verdict: FIXED, at the product, with the fixture left intact.** The previous
lane's deep-path fixture was not deleted, narrowed or made shallower. It is the
fixture the fix is proven against, and it is now 342 relative / 377 absolute
characters — deeper than when it was red.

### Root cause, reached by measurement after three wrong fixtures

The diagnostic discriminated between candidate causes rather than assuming one,
and it took **three corrections before the fixture could reach the defect at
all** — the "too clean to reach the defect" rule biting three times in one
investigation:

| Fixture attempt | Result | What was wrong with it |
|---|---|---|
| Deep path built with `\`, base `canonicalize()`d | all legs PASS at 306 chars | — |
| Same, separator made the variable | all legs PASS at 324/327 | The separator was never the variable |
| `canonicalize()` REMOVED from the base | **`atomic_write` FAILS, os error 3** | `canonicalize()` on Windows returns a `\\?\` **verbatim** path, so the fixture ran in the very mode that works |

At a 320-character **non-verbatim** absolute path on Windows 11 26200:

```
LONGPATH[control-native-sep]-LEN: 320
LONGPATH[control-native-sep]-CREATE-DIR-ALL: None          <- std OK
LONGPATH[control-native-sep]-STD-WRITE: None               <- std OK
LONGPATH[control-native-sep]-ATOMIC-WRITE: Some(Os { code: 3, kind: NotFound, ... })
```

`std::fs` applies its own long-path handling; the tempfile round trip inside
`atomic_write` reaches Win32 without it. **The defect was isolated to
`atomic_write`, and it was never about the separator or the depth.**

`LongPathsEnabled` is `0x1` on that box, so the registry was never the gap.

### The fix, and why this shape

Put to the four-way panel with the measurement in hand. Split **D / A / C** — no
majority. D and C agreed on substance (fix the write path AND preflight at
restore); A dissented that the write-path fix alone suffices. **Adopted D**, the
minority-by-count position, because it carried evidence the others did not
address: `\\?\` does not fix — and for reserved names makes *stricter* — a wider
class a portable archive can legitimately carry. D also subsumes C without a
manifest change, which matters because F26-03-A already flags the manifest as an
untyped string channel.

1. **`wcore_config::atomic_io`** resolves a long destination to extended-length
   form before the tempfile round trip. This lifts the limit at the Win32 layer
   **independently of `LongPathsEnabled`**, so it does not depend on machine
   configuration. Bounded by a length threshold, so short paths keep present
   behavior. Centralized in one function per the AGENTS.md platform rule.
2. **Restore REFUSES** paths this platform cannot materialize, before the first
   write, naming every offender. The target root is known there, so the
   objection is exact.
3. **Create only WARNS.** The creating machine cannot know which platform will
   restore. A Linux-to-Linux archive carrying `aux.txt` is entirely valid, and
   refusing it to prevent a hypothetical would break correct use.

### Live proof, both platforms, real release binary

**Windows** (`SeanD@seandesktop`, release binary built on the box at this SHA):

```
DEEP-CREATE-EXIT: 0
DEEP-RESTORE-EXIT: 0          <- was exit 1, "os error 3"
DEEP-SRC-DIGEST:    f418db934f9cdb3003be1ad0247c734fc08818d1ed8ffa5884ea48364e743214
DEEP-TARGET-DIGEST: f418db934f9cdb3003be1ad0247c734fc08818d1ed8ffa5884ea48364e743214
DEEP-RESTORED-ABS-LEN: 377
DEEP-CANARY-PRESENT: yes
```

That digest is **byte-identical to the Linux run's**, so the round trip is proven
equal across platforms, not merely self-consistent on each.

**The cross-platform refusal, with the archive built where those names are
legal.** An archive was created **on Linux** carrying `aux.txt` (a reserved DOS
device name) and `reports/report:final.md` (a forbidden character), transferred
to Windows (SHA-256 verified identical at both ends), and restored over a target
holding a LIVE profile:

```
HOSTILE-RESTORE-EXIT: 1
HOSTILE| refusing to restore into C:\f26c-work\hostile-target: 2 archived path(s)
HOSTILE|   cannot be written on this platform.
HOSTILE| - aux.txt: component 'aux.txt' is a reserved Windows device name ...
HOSTILE| - reports/report:final.md: component 'report:final.md' contains ':' ...
HOSTILE| ... Nothing has been written.
HOSTILE-PRE-DIGEST:  ee52957da0e2eceaa5307efcd788a75cb2bbfd16c4dfe658553f126747a2e25a
HOSTILE-POST-DIGEST: ee52957da0e2eceaa5307efcd788a75cb2bbfd16c4dfe658553f126747a2e25a
```

The target is **byte-identical after the refusal**, measured by digest rather
than read off the message. The live profile and a file the archive does not
contain both survive.

**Positive control (Linux)** — without it a Windows refusal would be equally
consistent with a simply-broken archive. The same archive restores **cleanly on
Linux**, both files present, and `create` warned about exactly 2 paths naming
both. A clean deep tree reports `windows_unrestorable_paths: 0`, so a
long-but-legal path is **not** refused — refusing it would "solve" F26-03-D by
declining to do the work.

## What F26-03-D unblocked: the Windows legs that had no result

All four now run. Three pass; one fails and is reported red.

| Windows leg | Before (lane 26b) | Now |
|---|---|---|
| Uncatchable kill (TerminateProcess) | **NOT RUN** — blocked by F26-03-D | **PASS** — mid-flight established, `DIGEST-EQUAL: yes` |
| Negative control (undersized) | NOT RUN | **PASS** — `late-kill-detected`, exit 9 |
| Open handle held during restore | NOT RUN | **PASS** — resolved cleanly, exact tree |
| Handler control (catchable mechanism) | NOT RUN | **FAIL** — see F26-03-E |

## Task 4 — both panels run, decided, bound to measurement

### Panel A — interruption non-vacuity: `CHOSEN: both-legs-sound`, BASIS: majority

Four-way **unanimous**. Binding gate PASSES.

| Measurement | Linux | Windows |
|---|---|---|
| `MIDFLIGHT-JOURNAL-OPEN` / `-TARGET-INTERMEDIATE` | yes / yes | yes / yes |
| `completed_before_kill` | no | no |
| `DIGEST-EQUAL` | yes | yes |
| Negative control detected its late kill | yes, exit 9 | yes, exit 9 |
| Handler control fired | **yes** | **no** |

The two `DIGEST-ALGO` lines are byte-equal, so the cross-platform comparison
measures content rather than encoding.

### Panel B — remap operability: `CHOSEN: remap-actionable`, BASIS: majority

Four-way **unanimous**. Binding gate PASSES across all four credential backends:
**0** unnamed fields, **0** surviving source absolute paths, **0** refusals that
wrote their target (measured by digest, not read off the message).

**A vote was nearly dropped silently.** Codex's first invocation blocked reading
stdin and returned 39 bytes with no verdict. Caught and re-run with stdin closed
rather than recorded as a three-way panel — exactly the failure mode the phase
warns about.

## Findings

**F26-03-E (MEDIUM, BACKLOG) — the Windows handler control does not fire, so
`fired=no` there is corroborated by Win32 semantics rather than instrumented.**
The probe is proven to ARM and the catchable console CTRL_C event is proven to be
DELIVERED (the helper exits 0; the target process dies, leaving
`DIGEST-EQUAL: no`), but the process is torn down before the probe file is
written. **Consequence stated exactly:** on Windows the rollback-exactness claim
is fully measured and holds; the separate claim that `TerminateProcess` is
*uncatchable* rests on documented Win32 semantics plus a delivered-but-unrecorded
event, not on a fired probe. None of the four load-bearing measurements depends
on the probe, which is why this is a control gap and not a vacuous leg.

### Instrument defects found and FIXED, all self-passing in shape

1. **Both proof scripts printed the literal string `installed=yes`.** The line
   asserted an armed probe whether or not one existed — and a probe that silently
   failed to arm produces exactly the `fired=no` the whole uncatchability claim
   rests on. It is now READ from a marker the binary writes only after a handler
   genuinely registers, and a probe that never armed is a hard failure.
2. **The Windows probe armed four handlers through a single irrefutable
   binding**, so one registration failure silently disarmed the entire probe.
   Each is now armed independently. This one was mine, introduced earlier in this
   same lane and caught by the same measurement.
3. `taskkill` without `/F` posts WM_CLOSE to a top-level window that an
   ssh-launched process does not have — nothing was ever delivered.
4. `CTRL_BREAK` killed the proof script itself (`STATUS_CONTROL_C_EXIT`), because
   `SetConsoleCtrlHandler(NULL,TRUE)` suppresses CTRL_C **only**.
5. `AttachConsole` RESETS the caller's standard handles, so doing it inline
   swapped the script's stdout away from the ssh pipe and produced **no output at
   all** — a run that looks like a hang. The console work now runs in a helper
   process that sacrifices its own handles.
6. The open-handle leg demanded a mid-flight kill it cannot have — the contended
   write fails fast, so the operation ends first — which scored a correct product
   outcome (exact rollback) as a harness failure.

## Gate results — real numbers

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked -p wcore-config -p wcore-cli --all-targets -- -D warnings`: **clean**.
- `cargo nextest run --locked -p wcore-config -p wcore-cli --no-fail-fast`:
  **2772 run, 2767 passed, 5 failed, 9 skipped, 1 flaky.**
  - 4 are `child_authority_corpus::corpus_{time,token,cost,depth}` — the known
    deliberate red (an unblinded canary). Not chased, not reported as regressions.
  - 1 is `wcore-config::hermeticity_audit_test::no_dirs_config_dir_bypasses_outside_canonical_helper`.
    **Verified pre-existing rather than assumed:** its own failure message names
    the offending line as `crates/wcore-gateway/src/service.rs:338` — a file this
    work never touches, in lane 24's area. Same finding 26-01 recorded (then at
    line 321; the file has drifted, the call has not).
  - **My delta: 0 new failures**, including in `wcore-config`, which I did modify.
- Long-path unit gate on Windows: RED at `d48f35b8` (os error 3) → GREEN at
  `e570bf90` on the identical fixture. A real red-to-green transition, not a
  green-from-the-start.
- Both interruption proof scripts self-red on a nonexistent binary → exit 1;
  remap capture script self-red → exit 1.
- No real-secret-shaped string under either panel directory, with the search
  first **proven non-vacuous** against a seeded control file.

## Deviations

1. **Branched from the tip of `plan/f20-unified-audit-repair` (`32e2f57d`)**, not
   the `56606bc4` named in the brief — the branch had advanced by two docs-only
   commits, and the tip carries the plan-gate linter fix.
2. **`crates/wcore-config/src/atomic_io.rs` modified**, outside 26-03's declared
   files. It is where F26-03-D actually lives; fixing it in `restore.rs` would
   have left every other `atomic_write` caller broken past `MAX_PATH`.
3. **`deep_path_over_max_path` exported `#[doc(hidden)] pub`** so the unit gate
   and the backup proof measure the same shape rather than two drifting
   approximations.
4. **No new dependency. `Cargo.toml` and `Cargo.lock` untouched.**

## Not done — stated plainly

- **Plans 26-02 (import/apply + quarantine) and 26-04 (hostile corpora) were not
  started.** No file of either was created. They remain wholly outstanding, and
  Phase 26 is therefore NOT complete.
- The prescribed credential-recovery action is **named** but never executed by any
  gate, so "the operator is told what to do" is proven and "following it produces
  a working install" is not.
- Every fixture remains synthetic and canary-seeded; no real home was archived.

---

# Predecessor record — lane 26b, at `2b139072`, retained in full

The following is the previous summary's body verbatim. It was accurate when
written; the Windows sections it marks NOT RUN or FAILING are superseded above.

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
