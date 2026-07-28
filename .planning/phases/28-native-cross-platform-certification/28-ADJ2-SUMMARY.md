# 28-ADJ2 — `F-28-ADJ-001` and `F-28-ADJ-002`

**`F-28-ADJ-001`: FIXED.** **`F-28-ADJ-002`: REPRODUCED, then FIXED.**

| | |
|---|---|
| Lane branch | `lane/28-adj2` |
| Base | `1b9f148f` (integration HEAD after six lanes merged) |
| HEAD | `9c4d2612` |
| Hardware | `SeanDesktop`, `C:\f28h2-repo`, rustc 1.95.0 |
| Evidence | `.planning/phases/28-native-cross-platform-certification/evidence/28-adj2/` |
| Suite | **136 passed / 0 failed / 23 ignored** (was 133/0/23) |

---

## 1. `F-28-ADJ-001` — the fourth self-passing gate, in my suite

**Filed correctly, and the root cause is exactly as stated.**
`reclamation_reports_grants_it_could_not_revoke` asserted only that the *quarantined file* still
contained the grant path. The file is **moved verbatim**, so that assertion holds no matter what
the operator is told. The test never called `reclamation_report`. The adjudicator's M3 deleted the
disclosure branch outright and the suite stayed at a byte-identical 133 passed / 0 failed.

**Repair.** Assert the report the product actually emitted, in **both directions**:

- a lease **with** un-revokable grants must name them, must say `could NOT be revoked
  automatically`, and must **not** claim `nothing was left behind`;
- a lease with **none** must say `nothing was left behind`, must manufacture no residual warning,
  and must name no path.

One direction alone is worthless here: disclosure-only is satisfied by an implementation that
always discloses, silence-only by one that never does.

**Why not just call the pure function.** That closes half the gap. `reclamation_report` is pure, so
testing it directly would not prove a *real* reclamation passes it the real lease — an
implementation that logged a constant would still pass. A `cfg(test)` recorder captures the exact
string handed to `tracing`, so the test observes what an operator reads. Mutant **M4** exists
solely to prove that: it emits a constant and the test fails.

I did **not** touch `a_leaked_test_lease_is_diagnosed_by_name`. As the coordinator noted, it pins
the message clause; the gap was the disclosure branch. Under M3 it correctly still passes —
confirming the two tests cover different things rather than overlapping.

## 2. `F-28-ADJ-002` — reproduced first, then fixed

The instruction was to reproduce before believing it. **It reproduces.**

`evidence/28-adj2/adj2repro-base.log` — base `1b9f148f`, binary `c732584c…`, `SRC_DIRTY=0`, real
product via `wayland-core sandbox status` / `sandbox exec`:

| Lease state | `sandbox exec` ran? | backend |
|---|---|---|
| clean | **yes** (`ADJ2RAN`) | `appcontainer` |
| 0-byte `.toml` | **no** | degraded to `fail_closed` |
| 0-byte, second run | **no** | `fail_closed` |

Diagnostic, verbatim: `invalid AppContainer ACL lease size 0 in \\?\C:\Users\seand\…\WCore-adj2-….toml`.
The clean row is the positive control, so the refusal is not a machine that was already wedged.
Effect **and** mechanism match the static reading: same permanent denial of service as
`F-28-02-002`, through a different door.

**Fix.** Reclaim the 0-byte lease through the **existing quarantine path** — no second recovery
concept, as instructed. The file is moved, not deleted, so an interrupted run stays visible.

**The safety argument, stated because it is the only thing separating this from deleting a live
writer's file.** The liveness gate cannot apply: an empty file carries no owner. The guarantee
comes from the mutation lock instead, and I verified it from the call graph rather than assuming
it — the **sole** production caller of `write_new_synced_lease` (`start_with_apply`, `acl_lease.rs:265`)
holds `MutationLock` acquired at `:256` across the whole create-then-write sequence, and
`recover_dead_leases_locked` runs under that same lock at `:257`. A 0-byte lease visible during
recovery therefore cannot belong to a running writer. This is the identical argument
`recover_rewrite_temps` already relies on to delete orphaned `.rewrite-*.tmp` files.

**Bounded deliberately.** Keyed on zero **length** only. A non-empty lease that will not parse is
indistinguishable from a tampered one and may carry real ACL grants, so it keeps failing closed.
`a_non_empty_unreadable_lease_still_fails_closed` pins that, and **M6** proves the pin is live.
**Residual I am not fixing and am not hiding:** a *partial* write (power loss after some bytes
reach disk) yields a non-empty unparseable lease, which still wedges. It is not safely
distinguishable from tampering, so widening to catch it would trade a denial of service for a
weakened security control. Named here rather than silently absorbed.

### Cause, not just effect

`zero_length_lease_is_reachable_through_the_writer` proves the 0-byte state is what the product's
**own writer** leaves on disk, by observing the file between `create_new_nofollow` and
`write_and_sync` — measured at that instant, not inferred from reading the source, and without
killing anything. The probe seam mirrors the crash-phase hook `rewrite_with_hook` already uses, so
it is this file's existing pattern rather than a new one; in production it is a closure that does
nothing.

### Live proof at HEAD

`evidence/28-adj2/adj2repro2-fix.log` — `9c4d2612`, binary `b3b235fc…`, `SRC_DIRTY=0`:

| Lease state | active | quarantined | ran? | backend |
|---|---|---|---|---|
| clean | 0 | 0 | **yes** | `appcontainer` |
| 0-byte `.toml` | 1 | 0 | **yes** — reclaimed in-flight | `appcontainer` |
| second run | 0 | 1 | **yes**, silently | `appcontainer` |

Operator text: `RECLAIMED a 0-byte AppContainer ACL lease …WCore-adj2-….toml. A lease file is
created before its content is written, so an execution interrupted in that window leaves an empty
file. This is persistent on-disk state — NOT a platform limitation and NOT transient — … MOVED
(not deleted) to …\quarantine\… so the interruption stays visible.`

## 3. Mutation battery — every checker run against a known-negative

`evidence/28-adj2/adj2-mut-m*.log`, `mutants.diff`. All eight target tests are resolved against
`--list` **before** running and the resolved count asserted (`8/8`), so a filter that matches
nothing cannot pass as a green. `MUT_COMPILED=True` on every mutant.

| Mutant | Restores | Result |
|---|---|---|
| **M3** (adjudicator's) | delete the residual disclosure branch | `reclamation_reports_grants_it_could_not_revoke` **FAILED** — previously green |
| **M4** | emit a constant instead of the real report | same test **FAILED** — proves the emit path is observed |
| **M5** | drop the 0-byte reclaim | `zero_length_lease_is_reclaimed_not_refused_forever` **FAILED** |
| **M6** | widen reclaim to any unreadable lease | `a_non_empty_unreadable_lease_still_fails_closed` **FAILED** |

Each mutant kills **exactly** its target and leaves the other seven green (`135 passed; 1 failed`
each time).

## 4. The fifth self-passing gate — mine again, and the same shape twice

You said to assume there was a fifth. There was, in this lane's own repro harness.

`adj2-repro.ps1` reported **`size_error=False`** while the raw log contained
`invalid AppContainer ACL lease size` **four times**. The console hard-wraps long lines, splitting
the phrase across a newline, so a literal `-match` misses it. Re-classifying the base log with a
whitespace-normalising matcher gives `count=4`.

**This is the same defect I recorded in `lane/28-h2` as an under-detecting `reclaimed=False`
marker — and I did not fix the harness, so it recurred.** Noting a gate defect without repairing
the instrument is not a fix. Under-detection is the dangerous direction: it reports the defect
**absent**, which is how a real wedge gets written off.

Repaired, and the repair is itself tested. `adj2-repro2.ps1` normalises whitespace before every
match and self-tests in the same run:
`CLASSIFIER_SELFTEST=known_positive=True;known_negative=True;old_matcher_missed_it=True`.
The third field is the part that matters — it proves the self-test is not vacuous by confirming the
old matcher genuinely missed the wrapped positive.

Running list, five now on record: (1) `--exact` filter matching no test name; (2) stale binary via
`Copy-Item` mtime preservation; (3) nested child test-process summary spliced in, first regex match
wins; (4) M3 — a test that never called the function it was named for; (5) a literal matcher
defeated by console line-wrapping. Adjudicator's own: `--list` regex anchoring `$` against trailing CRs.

## 5. Other gates

- **Clippy:** 4 warnings, 0 errors — **identical to the 4 measured at base `12fc794f`**, all
  `unused import` in `tests/hard_process_containment_windows.rs`, untouched by this lane. Zero new.
- **Live acceptance** (`--lib --ignored`, `WAYLAND_SANDBOX_LIVE_WINDOWS=1`): **20 passed, 3 failed.**
  All three are `required_live_bwrap_*` — Linux bubblewrap on Windows, `required live bwrap must be
  installed and usable` — already measured failing identically at base in lane `28-h2`. Pre-existing
  and environmental. Reported red, not silenced.
- One process failure worth recording: I ran `cargo fmt --check`, it returned **1**, and I committed
  anyway because the `git commit` was chained past it with `;` rather than `&&`. Caught on the next
  command and corrected in `9c4d2612`. The gate worked; I did not read it before acting.

## 6. What I did not do

- No merge, no PR — both reserved to Sean. No `wcore-contract generate`.
- No shared-file edits: `wcore-cli/src/lib.rs` and `main.rs` untouched.
- The ledger is not re-adjudicated here. Same reasoning as `28-h2`: this is evidence for that call,
  not the call.
- Box left as found — active leases `0`, quarantine directory removed, both archived artifacts in
  `C:\p22-evidence\stale-leases-backup` intact, 201 GB free.
