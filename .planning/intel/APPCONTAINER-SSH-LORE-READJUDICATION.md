# The SSH lore is refuted by its own control — and what that concealed

**Status: the lore is REFUTED, not merely doubted.** Measured 2026-07-27 on
`SeanD@seandesktop` at `455dd836`, over a non-interactive session-0 SSH logon.

This closes the open observation that Windows can run with the sandbox SILENTLY
DISABLED on an AppContainer ACL lease SID/profile mismatch. It also lists the
Windows sandbox reds the lore was used to discount, and says which now need
re-measurement.

Companion to `.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md`, which
established the mechanism. This file establishes the refutation and the
re-adjudication.

---

## 1. The lore, and the one observation holding it up

The standing rule, codified in `.planning/REQUIREMENTS.md:59` and repeated in
`HANDOFF-2026-07-26-autonomous-execution.md:102` and `22-03-PLAN.md:112,280`:

> "These ACL tests cannot be observed over SSH. A non-interactive session-0 SSH
> logon to `SEANDESKTOP` reports `AppContainerBackend::is_available() == false`,
> so every test in the file panics at its gate regardless of correctness.
> **Established by control**: the CI-certified-green
> `granted_path_is_readable_then_revoked` fails identically over SSH at the
> sealed SHA. Only the runner service is a valid environment — do not conclude a
> red from an SSH run."

The whole rule rests on that one control. **The control is non-discriminating.**
It varied the *logon* and held everything else fixed — but the actual causal
variable was the *state of the lease directory*, which was wedged on that box at
the time. Both hypotheses predict the identical outcome:

| Hypothesis | Predicts "CI-green test fails over SSH"? |
|---|---|
| A session-0 logon disables AppContainer | Yes |
| A stale unreconcilable lease disables AppContainer | Yes |

An observation both hypotheses predict cannot select between them. The control
was run correctly and reported honestly; it simply never had the power to
establish what was concluded from it.

## 2. The refutation, using the lore's own control

Same box, same logon class the lore says is disqualifying, lease directory
verified empty first:

```
LOGON: session=0 interactive=False ssh=True
LEASE_BEFORE=0
cargo test -p wcore-sandbox --test live_fs_acl -- --ignored --test-threads 1
```

```
test concurrent_allow_and_deny_identities_do_not_interfere ... ok
test denied_secret_under_granted_parent_is_unreadable_and_revoked ... ok
test deny_ace_still_blocks_granted_read ... ok
test granted_path_is_readable_then_revoked ... ok        <-- the lore's own control
test native_acceptance_gate_marker ... ok
test normal_sid_only_grant_is_denied ... ok
test one_execution_grant_never_leaks_to_another_identity ... ok
test one_exit_does_not_remove_another_execution_grant ... ok
test timeout_and_cancellation_remove_their_leases ... ok
test twenty_concurrent_executions_have_unique_temp_roots ... ok
test ungranted_path_is_denied_in_sandbox ... ok
test unrelated_acl_survives_exact_sid_cleanup ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 14.04s
LEASE_AFTER=0
```

**12/12 PASS over SSH, session 0, non-interactive** — on the exact file the rule
says "every test in the file panics at its gate regardless of correctness", and
including the exact test cited as the control.

The rule is false as written. The correct rule is: **check the lease directory
first. If it is empty, an SSH-observed red is a real red.**

## 3. Re-adjudication list

### A. Dismissed by the lore — the rule itself must go

| # | Item | Where | Now |
|---|---|---|---|
| A1 | `granted_path_is_readable_then_revoked` "fails identically over SSH" | `REQUIREMENTS.md:59`, `20A-04-SUMMARY.md` §13.10.2 | **RE-MEASURED, PASSES.** The observation was real; the inference was not. Its failure is fully explained by the wedged lease directory. |
| A2 | `live_fs_acl.rs`, all 12 tests, "panic at their gate regardless of correctness" over SSH | same | **RE-MEASURED, 12/12 PASS.** No re-run needed. |
| A3 | The standing instruction "never conclude a red from an SSH run" | `REQUIREMENTS.md:59`, `HANDOFF-2026-07-26:102`, `22-03-PLAN.md:112`, `22-03-PLAN.md:280` | **MUST BE DELETED/REPLACED.** This is the item with ongoing blast radius: it converts every future Windows sandbox red into an unfalsifiable environment excuse. Needs a serialized edit by the orchestrator — these are shared, cross-lane files. |

A1 and A2 are the good news: the tests were correct all along, and no product
defect was hiding behind them. What was hiding behind them was the rule.

### B. Same `is_available()` probe, different claim — one control each, LOW priority

| # | Item | Where | Assessment |
|---|---|---|---|
| B1 | Hosted `windows-2022` reports `is_available() == false`; Windows jobs routed to self-hosted `ferrox-win-msvc` | `20A-01-SUMMARY.md:41`, `20A-01-BASELINE.md:615`, `20-22-SUMMARY.md:33` | **Probably genuine, re-measure cheaply anyway.** Same real-spawn probe, so the same conflation is structurally possible — but a hosted runner starts with an empty lease directory, so the lease-wedge explanation does not fit. My honest read: this one is a real server-SKU property and will survive re-measurement. Worth one control run because it is cheap, not because it is suspect. |

### C. Explicitly NOT in this class — do not reopen

| # | Item | Why it is already sound |
|---|---|---|
| C1 | `job_close_reaps_detached_descendant_with_no_residue`, `breakaway_is_denied`, `active_process_cap_is_enforced` timing out | `20-VERIFICATION.md:43` root-caused these to WMI returning `CommandLine = NULL` for Low-IL AppContainer processes, and **explicitly disproved the SSH explanation** by reproducing under a SYSTEM-context scheduled task. Exemplary handling — the investigator did the discriminating control this lore never got. |
| C2 | `win-service-scm` excluded | `24-PHASE-REPORT.md:125` — an SCM handshake limitation of a console probe binary. Unrelated to the sandbox probe. |

### D. Surfaced by this work, not previously attributed

| # | Item | Status |
|---|---|---|
| D1 | `equivalent_windows_spellings_resolve_to_one_file_identity` RED on Windows | **FIXED at `455dd836`.** Had been failing on the `\\?\` leg; it is the leak that produced the wedging files. See §4. |
| D2 | Intermittent `dispatch admission refused: invalid retained workspace reservation` on clean-lease probes | **OPEN, not a sandbox failure.** 2 of 4 clean probes (`clean-1`, `clean-final`) failed this way with `disabled=0` and `mismatch=0` — the sandbox was available; dispatch admission refused for an unrelated reason. This is the "one honest wrinkle" the wedge intel recorded as a non-reproducing one-off; it has now reproduced twice, so it is not a one-off. Belongs to whoever owns swarm dispatch admission. |

## 4. Why the leases were there — the loop is now closed

The two wedging files were `WCore-storage-*-00000000000000f2.toml`. Tag `0xf2`
identifies exactly one producer: `test_lease(0xf2, …)` in
`equivalent_windows_spellings_resolve_to_one_file_identity`. Recorded state
`prepared` matches, and `sha256(b"storage-test-sid")` matches their `sid_sha256`
byte for byte.

That test was **already failing**, and its failure is the leak: it panicked at

```
open AppContainer ACL lease directory \\?\\\?\C:\Users\seand\AppData\Local\...\v1:
The filename, directory name, or volume label syntax is incorrect. (os error 123)
```

`lease_directory()` resolves through `GetFinalPathNameByHandleW` and therefore
already returns the verbatim `\\?\C:\…` spelling; the test prepended `\\?\` to it
again. The panic happened **before** the test's `remove_validated_lease` cleanup,
so the lease it had written stayed behind — in the production directory, where it
disabled the sandbox permanently.

The same wrong assumption also made the drive-letter leg dead code: byte 1 of a
verbatim path is `\`, not `:`, so that branch silently never ran.

Three defects, one root cause, and none of them visible while every red on that
box was being attributed to SSH.

## 5. What changed in the product

- `lease_directory()` resolves through `lease_root()`, which under `cfg(test)` is
  a per-process temp directory. Unit tests can no longer reach the production
  lease directory at all.
- A dead lease bearing the test SID sentinel is now **named** rather than
  reported as a generic mismatch, and the message states that the condition is
  persistent on-disk state, not a transient or environment fault. Live-verified
  against the archived wedging file: the running product emits
  *"…was written by wcore-sandbox's OWN TEST SUITE (it carries the test SID
  sentinel) and can NEVER reconcile against a real AppContainer profile."*
- No sandbox check was weakened. An unreconcilable lease is still refused, and
  the wedged probes still report `disabled=1`.

**Note for anyone re-running the probe:**
`.planning/intel/appcontainer-lease-wedge-probe.ps1` counts
`ACL lease SID/profile mismatch`. For a *test-origin* lease that string is now
replaced by the more specific message, so the probe reports `mismatch=0` while
still correctly reporting `disabled=1`. Match on `disabled=` for the wedge
signal, or add `OWN TEST SUITE` to the mismatch pattern.
