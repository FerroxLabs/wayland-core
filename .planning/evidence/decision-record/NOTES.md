# NOTES — lane/decision-record

Lane base: `c9ab048b952c5bc74c75ea8f76df06788408de59`.
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-decision-record` (asserted
via `/usr/bin/git rev-parse --show-toplevel`).

This is a writing/process lane. Deliverables: (1) a merged ADR on the
durable-session-vs-availability question, (2) a code-side pointer to it, (3) a process proposal.

All measurements below were captured by redirecting an absolute-path tool to a file and reading
the file with the Read tool, per LANE-BRIEF §3b.

---

## T+0 — brief premise verification (BEFORE writing anything)

The brief says "I have been wrong five times today." So every premise gets checked.

### Verified TRUE

| premise | how checked | result |
|---|---|---|
| `906287e1` = 2026-07-16 "feat(recovery): seal interrupted turn state" | `git log -1 --format` | **TRUE.** Full SHA `906287e1790ab2e0c8a6f1f71940e9acc2b55c75`, Thu Jul 16 12:54:28 2026 +0700 |
| its stated reason | commit body | **TRUE, verbatim**: "Make provider, tool, hook, budget, approval, and host recovery durable so restarts fail closed instead of replaying ambiguous effects." |
| `f14_sigkill_recovery.rs:1106` is the encoding test | Read tool, offset 1080 | **TRUE.** Line 1106 = `async fn isolated_profile_without_secure_store_fails_before_turn_or_provider_intent()`. File is 1913 lines. |
| `c73ac417` = today's change | `git log -1` | **TRUE.** Thu Jul 30 20:20:56 2026 +0700, `merge(fix-headless-keyring)`. |
| its quoted reason | commit body | **TRUE, verbatim**: "with no journal there is nothing at rest for that encryption to protect." |
| panel split 2-1 | commit body | **TRUE**: "Panel split DEGRADE 2 (codex, kimi) / REFUSE 1 (gemini)". |
| two independent testing lanes found the release blocker | commit body | **TRUE**: "Found independently by two UAT lanes from opposite directions." |
| test FAILS at integration head, panicking at `f14_sigkill_recovery.rs:204` | `lane/fix-tui-noise` captures (below) | **TRUE**, and measured by a lane that was not trying to prove it. |

### Verified FALSE — corrections

1. **`recovery_confidential.rs` has SIX variants, not five.** The brief says "a deliberate
   five-variant enum". Measured from source: `PlaintextBackendRejected` (:82),
   `NoSecureBackendAvailable` (:89), `SecureStoreUnreadable` (:95), `MissingRecoveryKey` (:100),
   `Unavailable` (:102), `Invalid` (:104). Six. The *characterization* is right — its doc comment
   names "live UAT defect D3: three unrelated causes rendered as one string" — but the last two
   (`Unavailable`, `Invalid`) are terse generic variants, so a reader counting only the richly
   worded ones gets five.

2. **The ADR location the brief suggests is not the repo's convention.** The brief says to look at
   `docs/design/` (35 dated design docs). But **`docs/decisions/` exists** and is a numbered ADR
   series: `0001-binary-size-v0.2.0-vs-v0.2.1.md`, `0002-performance-baseline-v0.2.1.md`. That is
   the ADR convention. This ADR goes to `docs/decisions/0003-*.md`.

3. **The panic at :204 is NOT the journal read.** Line 204 is the `ready`-handshake assertion in
   the `CoreProcess::launch*` helper — `assert_eq!(ready.get("session_id")…)`. The journal read is
   at :1143. So the test dies at *process launch*, before it ever reaches the journal assertions.
   This matters: the failure is that the `ready` frame **carries no `session_id` key at all**
   (`left: None`, `right: Some("f1400000000000000000000000000000")`), not that a file is missing.

---

## T+1 — the causation chain, as far as it is measured

Ancestry (all via `/usr/bin/git`):

- `b8311575` is the **first parent** of `c73ac417`. So it is exactly "integration immediately
  before the keyring merge" — the cleanest possible before-point.
- `git diff --stat b8311575 c73ac417 -- crates/wcore-cli/tests/f14_sigkill_recovery.rs` is
  **empty**. The fix did not touch the test. Any behaviour difference is the product's, not a
  test edit's.
- `c73ac417` is an ancestor of `e7bc6d88`, which is an ancestor of my base `c9ab048b`.

The AFTER half is measured, by `lane/fix-tui-noise`, at base `e7bc6d88` (fix present):

- `.planning/evidence/fix-tui-noise/captures/gates/test-f14-solo.log`:
  `test result: FAILED. 10 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out`
- `.planning/evidence/fix-tui-noise/captures/gates/test-f14-base.log`:
  `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 11 filtered out`
- Both panic at `crates/wcore-cli/tests/f14_sigkill_recovery.rs:204:9`, `left: None`,
  `right: Some("f1400000000000000000000000000000")`.

That lane went further than it needed to and **proved the red was not its own**: it reverted both
its source files to base, confirmed both crates actually recompiled (it says so explicitly,
because a silently-skipped rebuild would have measured its own binary and proved nothing), and
the test still failed. It also re-ran under `RUST_LOG=info`, which disables its new sink path
entirely — identical failure.

**STILL OPEN at T+1: the BEFORE half.** Nobody has recorded a run at `b8311575` showing
`1 passed`. The brief asserts it. I have not verified it. The test is `#[cfg(target_os = "linux")]`
so the Mac cannot run it at all — the §0 Darwin exception does not apply (this is Linux-only
behaviour, the exact inverse). It needs hetzner.

Until that run exists, the honest statement is: *the test fails with the fix present, on a test
file the fix did not modify, and the failure is not attributable to the lane that measured it.*
That is strong but it is not the same claim as "it passed before".

---

## Finding (unreported elsewhere so far) — the `ready` frame silently drops `session_id`

`c73ac417`'s own commit body lists, under "NOT DONE, and not claimed":

> `--json-stream` and Desktop hosts are NOT told: the notice goes to stderr and the decision
> precedes any protocol writer.

The f14 capture shows that is **understated**. Desktop hosts are not merely un-told — they are
told something *wrong and unannounced*: the `ready` event, which previously carried
`session_id`, now omits the key entirely on a keyring-less host. The full `ready` payload in the
capture has `capabilities`, `contract`, `execution_policy`, `type`, `version` — and no
`session_id`. A host that reads `ready.session_id` gets `undefined` with no explanation
anywhere in the protocol stream.

So the degraded mode IS observable over the wire; it is just observable as a missing field
rather than as a declared state. That is the worst of both options — a silent contract change.
`durable_sessions_disabled_by_host()` exists (per the same commit) with no consumer; this is the
consumer-shaped hole it was created for.

---

## T+2 — the causation run (RESOLVED, measured)

Ran on `hetzner-dsm` in a fresh worktree `/root/wayland-decision-record` at `b8311575`. Load at
launch 5.78, 913G free — a single targeted `-p wcore-cli --test f14_sigkill_recovery` is within
LANE-BRIEF §2. Full binary, **no name filter** (§3.2 flavour (c)).

Anti-vacuity: 672 crates compiled from source including `wcore-cli` and `wcore-agent` from
`/root/wayland-decision-record/crates/`; the script self-reports HEAD into the log; `WLRC`/`WLDONE`
sentinels written and read back from a file; output copied locally and read with the Read tool,
never through a Bash pipe (§3b).

```
=== HEAD self-report ===
b831157508a2a0598d7a5f380cc49c4a9477f848
...
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 44s
     Running tests/f14_sigkill_recovery.rs (target/debug/deps/f14_sigkill_recovery-0c2c7646b8296b33)
running 12 tests
test isolated_profile_without_secure_store_fails_before_turn_or_provider_intent ... ok
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 14.54s
WLRC=0
WLDONE
```

Against `lane/fix-tui-noise`'s AFTER run at `e7bc6d88` (fix present), same binary hash
`0c2c7646b8296b33`: `FAILED. 10 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out`.

**Exactly one test flipped, on a test file the fix did not modify. Causation established.**
This also closes `lane/durable-posture`'s open item — their premise 5 was marked UNVERIFIED
pending hetzner; it is now verified, and their worktree does not need to repeat it.

## T+3 — the merge-cadence premise is FALSE as stated

Brief: "the merge cadence gates on `fmt` + `metadata --locked` + `check --workspace --all-targets`
and runs **no tests and no clippy**".

Measured from `.planning/evidence/fix-headless-keyring/NOTES.md:177-186` — the keyring lane ran
`test -p wcore-config --lib` (579 passed), `test -p wcore-agent` (2 targets), `test -p wcore-cli
--test json_stream_startup_refusal` (6 passed), scoped clippy (0,0,0), and workspace clippy (101,
proven pre-existing). **Tests ran. Clippy ran.**

The real mechanism is finer and is what ADR 0004 §3 argues from:

- every test that ran was scoped to a crate the author **changed** (`wcore-config`,
  `wcore-agent`), plus one integration target hand-picked because the author knew it was about
  refusal;
- the broken test lives in `wcore-cli`, whose **source the change never touched**;
- the only workspace-wide gate was `cargo check --workspace --all-targets`, which **compiles
  everything and runs nothing**. The test compiled perfectly.

Also note `.gate.sh` in the repo root *does* run fmt + clippy + nextest, but its `BRANCH` is
hardcoded to `feat/issue-158-plan-tier` — a stale artifact of a different branch, not the current
cadence.

## T+4 — Tier-2 discovery mechanism, executed against the real case

```
$ git log -S "isolated_profile_without_secure_store_fails_before_turn_or_provider_intent" \
      --format='%h %ad %s' --date=short -- crates/wcore-cli/tests/f14_sigkill_recovery.rs
906287e1 2026-07-16 feat(recovery): seal interrupted turn state
```

One line, the exact introducing commit, reasoning in its body. The proposed rule is tested, not
aspirational.

False-negative surface, measured (`/usr/bin/grep -rn` over `crates/`, control-negative 0,
control-positive 5035): **250** `#[ignore]` occurrences, **13819** test attributes, **552**
integration test files. Plus the specific trap in this case — the test is
`#[cfg(target_os = "linux")]`, so a macOS-only gate would never have compiled it.

---

# OUTCOME

**All three tasks landed.** Lane branch `lane/decision-record`, HEAD
`5825807e1969129784a19f0d103a9b775e8dd583`, pushed to `gh` (remote SHA asserted equal).

| # | deliverable | state |
|---|---|---|
| 1 | `docs/decisions/0003-durable-sessions-without-a-secure-store.md` — the merged ADR | landed |
| 2 | Doc-comment anchor on `RecoveryConfidentialError`, `crates/wcore-agent/src/recovery_confidential.rs` | landed |
| 3 | `docs/decisions/0004-cross-audit-panels-must-see-prior-decisions.md` — the process fix | landed |

**ADR location:** the brief suggested `docs/design/`. The repo's actual ADR convention is
`docs/decisions/NNNN-*.md` (`0001-binary-size-v0.2.0-vs-v0.2.1.md`,
`0002-performance-baseline-v0.2.1.md`). Used that; mine are 0003 and 0004.

**Gates.** `cargo fmt --all -- --check` on the Mac → **rc=0** (permitted by §0).
`cargo clippy -p wcore-agent --lib -- -D warnings` on hetzner at HEAD `5825807e` → **rc=0**,
with `Checking wcore-agent` present in the log so the crate genuinely recompiled. No test run
was needed for a doc-comment change; the causation run above is the lane's substantive measurement.

**Deviations.** My first two commits carry author `ci <ci@ferroxlabs.dev>` because I passed `-c`
explicitly; the repo default is `ci <sean@seandonahoe.com>`, which the third commit uses. Not
corrected — `rebase`/`reset` are forbidden by §0 and the cost of the inconsistency is zero.

**Verdict: criteria met.** The causation claim is measured on both sides rather than taken on
trust, and three of the brief's premises were refuted with evidence.

**Still open (carried into ADR 0003 §7, not resolved here):**
- the `ready` frame silently dropping `session_id` is unassigned and needs a seam request;
- `durable_sessions_disabled_by_host()` still has no consumer;
- `switch_active_session` (journal writer #3) is still unguarded;
- the second panel's transcript is not committed anywhere in the repo — ADR 0003 §5 records that
  caveat rather than presenting relayed content as measured.
