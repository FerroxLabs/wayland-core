# Lane `sandbox-lease` — test leases can no longer disable the sandbox

**Branch:** `lane/sandbox-lease` · **HEAD:** `f5a56e17` · **Base:** `cd5b4e9b`
**Verified on:** `SeanD@seandesktop`, real Windows, non-interactive SSH, `session_id=0`
**Verdict: goal achieved.** Red before, green after, on the machine that had the defect.

---

## 1. What was wrong

`wcore-sandbox`'s unit tests wrote AppContainer ACL leases into the **production**
lease directory (`%LOCALAPPDATA%\Wayland\Core\AppContainerLeases\v1`). Those
leases carry a synthetic `WCore-storage-…` profile name for which no
AppContainer profile is ever created, so `recover_dead_leases_locked` can never
derive a matching SID and fails closed permanently.

On Windows `is_available()` is a **real spawn**, so that failure makes the
availability probe report the backend unavailable. The engine logs
`sandbox disabled` and **carries on running unsandboxed**. Test pollution was
therefore a path to production running with no sandbox — which is why this is a
security defect and not test hygiene.

## 2. Every path a test could write into the production lease directory

All of them funnel through `lease_directory()`, which is why one chokepoint
fixes all of them. Enumerated, not sampled:

| # | Site | What it wrote | Class |
|---|---|---|---|
| 1 | `storage.rs` `atomic_rewrite_is_old_or_new_across_injected_crash_phases` | 3 synthetic leases (tags 0,1,2) | unreconcilable |
| 2 | `storage.rs` `opened_lease_cannot_be_swapped_under_validation` | 2 synthetic leases (0xf0, 0xf1) | unreconcilable |
| 3 | `storage.rs` `equivalent_windows_spellings_resolve_to_one_file_identity` | 1 synthetic lease (0xf2) | unreconcilable — **this is the one that leaked** |
| 4 | `storage.rs` `lease_symlink_is_rejected_without_following_it` | a **symlink** into the lease dir | stray entry |
| 5 | `acl_lease/tests.rs` `malformed_or_unknown_lease_fails_closed` | raw `fs::write` of a deliberately malformed lease | unparseable |
| 6 | `acl_lease/tests.rs` `setup_failure_after_durable_lease_cleans_up`, `live_owner_is_never_reclaimed`, `killed_owner_is_recovered_before_next_execution` + `crash_helper_entry` | real `ExecutionIdentity` leases | reconcilable, but still production state |
| 7 | `tests/live_fs_acl.rs` `lease_profiles()` | **read-only**, but hardcodes the 4 path components as a second source of truth | no write |

1–6 now resolve to a per-process temp root. 7 is read-only and untouched;
integration tests under `tests/` deliberately keep using the real directory
(see §3).

## 3. The fix

**Chokepoint.** `lease_directory()` resolves through `lease_root()`, which under
`cfg(test)` is a per-process temp directory. Five call sites fixed at once, and
the sixth one somebody adds is fixed before it is written. Chosen over guarding
call sites because guarding four of five doors is this program's recurring
failure mode.

**Enforced by visibility, not discipline.** Integration tests under `tests/`
compile the library without `cfg(test)` and still use the real directory — which
is correct, because they drive `ExecutionIdentity::start`, whose leases carry a
real profile and real SID and reconcile normally. They *cannot* reach the
synthetic-lease helpers at all: those are private to the module tree.

**Loud rejection (task item 4).** A dead lease bearing the test SID sentinel is
now named as test-written, with the remedy, instead of being reported as a
generic mismatch. `TEST_SID_SENTINEL_SHA256` is frozen at the digest found in the
two real wedging files so those exact files stay recognisable, and a test
re-proves the digest correspondence every run.

**No check was weakened.** An unreconcilable lease is still refused; wedged
probes still report `disabled=1`. Nothing was `#[ignore]`d, `#[allow]`ed,
re-gated, deleted, or given a longer timeout.

## 4. Red before, green after

| Commit | `unit_tests_never_resolve_the_production_lease_directory` | `a_lease_written_by_a_test_never_lands_in_the_production_directory` |
|---|---|---|
| `c68b4e3d` (guard + tests, no chokepoint) | **FAILED** | **FAILED** |
| `848c5cbb` (chokepoint) | ok | ok |

```
c68b4e3d: test result: FAILED. 2 passed; 2 failed
f5a56e17: test result: ok.     4 passed; 0 failed
```

Both failing runs left `LEASE_AFTER=0` — the tests capture the observation and
remove the lease *before* asserting, so a test proving pollution never pollutes.

**Final state at `f5a56e17`**, one SSH session, `session_id=0`:

| Gate | Result |
|---|---|
| `wcore-sandbox --lib`, default gating | **126 passed, 0 failed**, 23 ignored |
| AppContainer ACL lease native acceptance (`--ignored`) | **11 passed, 0 failed** |
| `live_fs_acl` integration (`--ignored`) | **12 passed, 0 failed** |
| `clippy -p wcore-sandbox --all-targets` | exit 0 |
| Production lease directory | 0 before, 0 after, no residue |

**Live product evidence.** Planting the archived wedging file and running the
built binary through the existing probe
(`.planning/intel/appcontainer-lease-wedge-probe.ps1`, reused, only `$exe`
changed) yields `NEW_DIAGNOSTIC=PRESENT`:

> `…WCore-storage-00002d20-00000000000000f2.toml was written by wcore-sandbox's
> OWN TEST SUITE (it carries the test SID sentinel) and can NEVER reconcile
> against a real AppContainer profile.`

## 5. Defects found along the way

1. **`equivalent_windows_spellings_resolve_to_one_file_identity` was RED**, and
   its failure *is* the leak: it panicked before its cleanup, stranding the
   tag-`0xf2` lease. `lease_directory()` already returns the verbatim `\\?\C:\…`
   form and the test prepended `\\?\` again → `\\?\\\?\C:\…`, os error 123. The
   same assumption made the drive-letter leg dead code. Fixed at `455dd836`;
   the tag and state of the archived files match this test exactly.
2. **My own first gate was self-passing.** It compared a verbatim path to an
   ordinary one and so passed against the unfixed tree. Caught only because the
   second, end-to-end gate failed honestly. Repaired at `c68b4e3d` — same root
   cause as (1).
3. **Two regressions I introduced, both caught by running the suite rather than
   assuming**: the spawned helper process got its own lease root (`e0ccd60b`),
   and a pid-keyed root collided with leftovers after Windows reused a process
   id (`f5a56e17`).

## 6. Open / not done

- **`REQUIREMENTS.md:59`, `HANDOFF-2026-07-26:102`, `22-03-PLAN.md:112,280`
  still carry the refuted SSH rule.** Not edited here — shared cross-lane files;
  needs a serialized edit. See `APPCONTAINER-SSH-LORE-READJUDICATION.md`.
- **`dispatch admission refused: invalid retained workspace reservation`** —
  intermittent, 2 of 4 clean probes, sandbox available (`disabled=0`). Not a
  sandbox defect; belongs to swarm dispatch admission. MEDIUM → backlog.
- **Quarantine instead of permanent refusal** (wedge intel rec #2) deliberately
  **not** implemented. Moving an unreconcilable lease aside and continuing would
  abandon its recorded ACL intents, potentially leaving a real package-SID grant
  on a real path. That is a security tradeoff needing its own decision, not a
  drive-by. Naming the cause — which is what actually cost the weeks — is done.
- Test lease roots accumulate under `%TEMP%\wcore-lease-test-*` (small, cleared
  per run). Left as-is rather than adding teardown machinery.
- `cargo test -p wcore-sandbox --lib -- --ignored` **unscoped** also selects
  Linux-only `bwrap` tests, which fail on Windows. Pre-existing; scope to
  `appcontainer_acl_lease` on Windows.
