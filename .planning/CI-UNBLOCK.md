# CI-UNBLOCK — restoring the workspace test signal

Lane `lane/ci-unblock`. Base `plan/f20-unified-audit-repair` @ `ef1d97be`
(captured once, quoted throughout). Final commit `7f5c0455`.

**Headline: the blocker was not four lines. It was four lines plus three more
blockers stacked behind them, three of which are `cfg(windows)`-only and
therefore invisible to the Linux and macOS legs.** Clippy stops at the first
crate that fails, so each fix only revealed the next one. All four are fixed,
none is suppressed, and clippy is now green on Linux **and** Windows.

---

## 1. The four blockers

| # | Crate / file | Lint | Visible on |
|---|---|---|---|
| 1 | `wcore-eval-scenarios/src/journey.rs` :683,695,707,717 | `clippy::cloned_ref_to_slice_refs` | all 3 platforms |
| 2 | `wcore-cron/src/lease.rs:450` | `clippy::unnecessary_cast` | **Windows only** |
| 3 | `wcore-sandbox/tests/hard_process_containment_windows.rs` :34,39,43-45 | `unused_imports` ×3 | **Windows only** |
| 4 | `wcore-agent/tests/goal_fleet_wire_test.rs:175` | `dead_code` | **Windows only** |

CI's clippy step is `vx just lint` (`ci.yml:160`) → `cargo clippy --workspace
--all-targets -- -D warnings` (`justfile:75-76`); the linux-containerized job
runs identical flags at `ci.yml:323-324`. That step **precedes** the test step,
so `nextest --workspace` had executed **zero times** in CI on this tree since
2026-07-25.

**#1 — `cloned_ref_to_slice_refs`, new in Rust 1.95.0.** A toolchain-bump lint,
not a code regression, which is why it hit all three platforms at once and why
no lane felt ownership of it. `&[canary.clone()]` allocates a one-element array
by cloning the String; `std::slice::from_ref(&canary)` yields the same
`&[String]` by borrowing. The lint's own suggestion, and already the idiom in
this workspace including the same crate (`providers.rs:273,277,279,280`,
`compact/estimate.rs:239,245,251`, `acp/server.rs:1410`, `smtp.rs:628`).
Site 707 was the real risk — `canary` is moved into the expected value in the
*same* `assert_eq!`, so swapping a clone for a borrow puts a live borrow and a
move in one statement. Flagged as a risk before compiling, then confirmed to
compile; NLL ends the borrow when `scan_canaries` returns an owned `Result`.

**#2 — no-op raw-pointer cast.** `AsRawHandle::as_raw_handle` already returns
`RawHandle` = `*mut c_void`, exactly the `h_file` parameter type of the
`LockFileEx` extern, so `as *mut c_void` converted nothing. `c_void` stays
imported and used by `Overlapped::h_event` and the extern signature, so
removing the cast does **not** orphan the import into a follow-on
`unused_imports` error — the trap that would have cost another full CI round.

**#3 — imports orphaned by a same-day test rebuild.** All six identifiers were
verified to occur ONLY on their own import lines; the one other mention of
`SandboxManifest`/`SandboxCommand` is prose in the module doc comment at line
10. They were orphaned by `ceae23b4` ("rebuild the KR-01 reap test on
primitives that work") and `6c4871fc`, **both 2026-07-28 — inside the blind
window.** The file is `#![cfg(windows)]`, so off Windows it compiles to nothing
and the other legs cannot see it. No test deleted, no assertion changed, no
`#[ignore]` added; `NATIVE_CONTAINMENT_CASES` and its zero-execution guard are
untouched.

**#4 — genuinely platform-conditional dead code.** `Fixture::journal_path` is
read only by `a_second_opener_is_refused_the_writer_lease_on_unix`, which is
`#[cfg(unix)]` because the journal writer lease is a Unix-only construction
(threat T-22-06 — the test's own doc comment says so). Gated the field and its
initializer to `cfg(unix)` so it exists exactly where something reads it.
Chosen over `#[allow(dead_code)]`, which would have silenced the report while
keeping the dead weight, and over deleting the field, which would have broken
the Unix test.

**Nothing is suppressed.** No `#[allow]` of any kind was added, and no
`#[allow(clippy::cloned_ref_to_slice_refs)]` exists anywhere in `crates/`.

### The gate is proven able to fail

Per LANE-BRIEF §6b-ii, three assertions, not two. A script restores the pre-fix
`journey.rs` from base, runs clippy, restores the fixed file, runs clippy again:

| assertion | result |
|---|---|
| known-negative — pre-fix file | `WLNEG=101` FAILS |
| known-positive — fixed file | `WLPOS=0` PASSES |
| worktree restored, no residue | `WLDIFF=0` |

It failed for the *right* reason: the negative log holds exactly four
`cloned_ref_to_slice_refs` errors at exactly `journey.rs:683:13, 695:13,
707:34, 717:13` — same lines, same columns as CI log `30369041140`.

**Toolchain checked before trusting any of it.** The lint is 1.95-specific, so
an older rustc would have produced a green proving nothing (§3.2, "the gate was
already green at base"). Inside the worktree rustup honours the
`rust-toolchain.toml` pin: `rustc 1.95.0` / `clippy 0.1.95`, matching `vx.toml`.
Hetzner's *default* is 1.96.0 — checked from outside the worktree it would have
been the wrong compiler.

### Both platforms verified at the final commit `7f5c0455`

```
Linux  (hetzner-dsm)  cargo clippy --workspace --all-targets -- -D warnings
                      WLCLIPPY=0   error_lines=0
                      49 wcore crates + 5 wayland plugins = the 54-crate workspace

Windows (SeanDesktop) cargo clippy --workspace --all-targets -- -D warnings
                      WLRC=0       (WLDIRTY=1 confirms the patch under test was live)
```

Coverage counted, not assumed — rc=0 on a run that checked nothing is the exact
failure class this program keeps hitting. The one `warning:` line is cargo's
future-incompat notice for third-party `imap-proto v0.10.2`; not a clippy
diagnostic, not gated by `-D warnings`.

Windows was verified **directly on `SeanD@seandesktop`** rather than one lint
per CI round. Blockers #2, #3 and #4 were each invisible until the previous was
fixed; discovering them through CI would have cost three more full queue cycles.

---

## 2. What `nextest --workspace` reveals

**This had not run in CI on this tree for four days. It runs now.**

`cargo nextest run --workspace --profile ci --no-fail-fast`, preceded by the
same two pre-builds CI does (`tool_token_bench`, `wcore-cli --release`; both
rc=0), on `hetzner-dsm` @ `7f5c0455`:

```
Starting 12775 tests across 544 binaries (50 tests skipped)
Summary [76.329s] 12775 tests run: 12772 passed (4 flaky), 3 failed, 50 skipped
```

**The headline is that it is nearly clean.** Four days of unwitnessed merges
produced **three** failures, all explicable. Nothing here suggests broad rot.

Run twice — at `1e1770d4` (3 failed, 2 flaky) and at `7f5c0455` (3 failed, 4
flaky). **The same three failures both times**, so none of the four fixes
introduced anything. The flaky set differs between runs, which is what flaky
means; all are wall-clock/process-timing shaped.

### None of the three is mine — proven, not asserted

```
BASE=ef1d97beb61f1b084bdfba745e8f49830924d757
git diff "$BASE" --stat HEAD   ->  4 source files, all lint-only:
  wcore-eval-scenarios/src/journey.rs            (12 lines, #[cfg(test)] only)
  wcore-cron/src/lease.rs                        (1 line, cast removed)
  wcore-sandbox/tests/hard_..._windows.rs        (imports only)
  wcore-agent/tests/goal_fleet_wire_test.rs      (cfg gate only)
untracked files: 0   (--name-only is blind to these, so counted separately)
```

No assertion, no test body and no production behaviour is touched anywhere in
this lane.

### FAIL 1 + 2 — `browser_suite` / `computer_use` capability assertions (HIGH, dispatch)

| | |
|---|---|
| `wcore-cli::plugin_discovery_e2e` | `ready_event_has_plugin_capability_flags` (TRY 3 FAIL) |
| `wcore-cli::release_binary_smoke` | `release_binary_ready_event_advertises_plugin_capabilities` (FAIL) |

```
assertion `left == right` failed: expected capabilities.browser_suite=true
(wayland-browser plugin not discovered)
  left: Null      right: true
```

**The engine is right and the tests are stale.** Commit **`85b60a2f`
(2026-07-28) — `fix(agent): advertise browser/CUA capabilities on liveness, not
linkage`**, ledger row 27-C2(b), deliberately stopped advertising these flags
when no backend can actually start:

```
WARN not advertising browser_suite: the plugin is loaded but no backend can start
  reason=no browser backend can start: `camofox-browser` does not resolve on PATH
  and no sidecar answered http://localhost:9377/health
```

(`crates/wcore-agent/src/output/protocol_sink.rs:199` and `:212`.)

That is a *good* commit — it fixes a real defect where a headless host
advertised `browser_suite: true` and the first operation died with `spawn
camoufox: No such file or directory`. It shipped its own passing test
(`capability_liveness_narrowing.rs`), and `wcore-agent`'s
`browser_suite_advertised_when_wayland_browser_loaded` still passes. What it did
**not** do is update the two `wcore-cli` e2e tests, which have asserted these
flags unconditionally since `da5a18b5` (2026-06-08).

**It merged inside the blind window, and clippy being red is the only reason
nobody saw the two reds it created.** This is precisely the class of breakage
this lane was opened to expose.

The release test's own error text speculates "wayland-browser stripped by
release LTO?". **That hypothesis is dead** — the debug build fails identically.
Whoever picks this up should not spend time on LTO.

**Not fixed here, deliberately.** The repair is a contract judgement — make the
tests tolerate a liveness-narrowed capability on a headless host, provision
`camofox-browser` on runners, or force the capability under a test env var.
Each changes what these tests certify, and LANE-BRIEF §5 forbids weakening a
test to reach green. This belongs to the owner of 27-C2(b) with a cross-audit,
not to a drive-by from the CI-unblock lane. **They will fail on any runner
without `camofox-browser` installed, so expect them red in CI.**

### FAIL 3 — `desktop_contract_corpus` (MEDIUM, known, orchestrator-level)

```
drifted=["adversarial/events/fixture-mismatch.jsonl",
         "adversarial/events/schema-mismatch.jsonl",
         "adversarial/events/version-mismatch.jsonl",
         "events/ready.json", "manifest.json"]
```

Already documented as **CLASS-CONTRACT-01** in `.planning/BACKLOG.md:889`
("reddens every lane by construction — MEDIUM, friction not defect").
`SOURCE_INPUTS` digests 40 engine files including `wcore-cli/src/main.rs`, the
shared fence every lane is told to edit. I touched neither `main.rs` nor any
`SOURCE_INPUTS` file (`wcore-eval-scenarios` is not in the list — grepped, zero
hits), so this red is inherited from the integration branch.

Correctly **not** actioned: LANE-BRIEF forbids `wcore-contract generate`, and
BACKLOG is explicit that per-lane regeneration is actively harmful.

> **Seam request (orchestrator, serialize):** one `wcore-contract generate` over
> the merged tree once lanes are integrated, plus the matching Desktop re-pin.
> `schema_digest` is unchanged; only `fixture_digest` and `source_inputs_digest`
> move — the benign shape Sean authorized at `c743f398` / `5f74d559`.

### Flaky (passed on retry — recorded, not chased)

`wcore-swarm status_output_cap_kills_git_descendant`, `wcore-cli
deterministic_openai_loop packaged_core_cancels_an_active_stream`,
`wcore-eval-scenarios outer_deadline_reaps_owned_descendant_listener`,
`wcore-cli migrate_hermes import_is_idempotent_without_overwrite`. All
timing-shaped, matching the known contention class in BACKLOG.

### What did *not* go wrong

No EMFILE cluster in `wcore-skills` (`fs.inotify.max_user_instances` already
512, load 8.8/96 cores, 691G free), so no isolation re-run was needed. The
`no-tests = "fail"` nextest setting means a zero-test invocation could not have
passed silently. 544 binaries and 12,775 executed tests are counted from the
run, not inferred from exit status.

---

## 3. Two corrections to my own instruments

Recorded rather than left for the next lane to rediscover (§6b-ii: a documented
instrument defect is one you have agreed to keep).

**(a) My CI poll loop silently lost the Windows result.** Nested quoting through
`ssh → powershell -Command` mangled the probe, and because the loop suppressed
stderr the field simply rendered empty — indistinguishable from "not finished
yet". I concluded the Windows run had not started when it had in fact completed.
**Repaired**, not just noted: probes now run from an scp'd `.ps1` file so no
quoting crosses the ssh boundary, and the completion check greps for a `WLSHA=`
that must match the commit under test, so a stale or empty read cannot read as
success.

**(b) I stated the wrong cancellation rule, then measured it.** I read
`cancel-in-progress: ${{ github.event_name == 'pull_request' }}` (`ci.yml:47-53`)
and concluded a branch push never cancels. **That is only true of *in-progress*
runs.** GitHub also supersedes *pending* runs in the same concurrency group, and
runs `30394434006` (`25ab51c5`) and `30395656358` (`c085aa01`) were both
cancelled that way by my subsequent pushes. The in-progress run `30392102087`
survived every push, confirming the distinction. Consequence for the next lane:
**batch your pushes** — each one supersedes whatever is still pending.

---

## 4. CI run

`ci.yml` triggers on `lane/**`. Runner capacity was saturated throughout
(26 queued behind ~20 parallel lane branches).

| run | commit | contains | result |
|---|---|---|---|
| `30392102087` | `1e1770d4` | fix #1 only | **honest red** — see below |
| `30394434006` | `25ab51c5` | docs | cancelled (superseded pending) |
| `30394618000` | `c0404ad9` | fixes #1-2 | cancelled (superseded pending) |
| `30395656358` | `c085aa01` | fixes #1-3 | cancelled (superseded pending) |
| `30396126556` | `7f5c0455` | **all four fixes** | final — verdict in §5 |

**Run `30392102087` is the measurement that matters most, and it is the first
CI run on this branch to get past clippy on any leg.** It proved fix #1 works
and exposed blocker #2:

- `Browser live e2e (chromium)` — **success**
- `Eval acceptance gate (Linux, containerized)` — **success**
- `Build` ×5 targets (linux gnu x86_64/aarch64, windows-msvc x86_64/aarch64,
  darwin aarch64) — **success**
- `CI (Array)` (self-hosted Windows) — **failure at `Clippy (warnings = errors)`**,
  no longer on `journey.rs` but on `wcore-cron/src/lease.rs:450`

That is an honestly-failed run, reported red, with the failure read rather than
counted — and it is what turned a four-line lane into a four-blocker one.

---

## 5. Verdict — the signal is restored, and it is red for reasons worth having

**Goal ACHIEVED.** `Clippy (warnings = errors): completed success` and
`Run tests (nextest CI profile): completed failure` on `CI
(linux-containerized)` in run `30392102087`. **The test step executed in CI for
the first time since 2026-07-25.** Step-level proof, not a summary:

```
 7 Check formatting:                completed success
 8 Clippy (warnings = errors):      completed success   <- was failing since 07-25
 9 Run tests (nextest CI profile):  completed failure   <- had never run at all
13 Upload nextest JUnit report:     completed success
```

Everything after clippy is now reachable. Builds passed on all five targets,
`Browser live e2e (chromium)` and the `Eval acceptance gate` both passed.

### A fifth finding, visible only in CI

CI's containerized leg surfaced a failure hetzner did **not**:

```
wcore-agent::anvil_forge_transaction
  production_landing::drive_climb_full_lands_the_winner_surface_for_accept
  (TRY 3 FAIL, anvil_forge_transaction.rs:710)

GateUnrunnable("`true`: gate could not be spawned: sandbox UNAVAILABLE and
unsandboxed execution is not permitted — refusing to run with host permissions.
Install bubblewrap (Linux), set WAYLAND_SANDBOX=docker, or explicitly opt in
with WAYLAND_ALLOW_NO_SANDBOX=1")
```

**Environment gap, not a code defect** — and measured on both sides rather than
inferred: hetzner has `bubblewrap 0.9.0` at `/usr/bin/bwrap` and the same test
**PASSES** there (`PASS [2.418s] (3020/12775)`). The CI image has no usable
sandbox, so the gate cannot spawn. The engine's refusal is correct behaviour;
what is wrong is that the test hard-panics in that environment instead of
self-qualifying the way the live sandbox tests do. **Dispatch:** either install
bubblewrap in the CI image or give this test the same qualify-or-skip treatment
`require_live_windows` uses. Do **not** set `WAYLAND_ALLOW_NO_SANDBOX=1` in CI
to make it green — that disables the isolation the test exists to prove.

### The containerized leg cannot report a full failure set

`ci.yml:323` runs `$DOCKER_RUN "$CI_IMAGE" cargo nextest run --workspace
--profile ci` — **without `--no-fail-fast`**, unlike the `just test-ci` recipe
(`justfile:34-35`) the macOS and Windows legs use. Verified against line-wrap
rather than assumed. Consequence, measured:

```
CI (containerized):  Summary [58.0s]  2348/12775 tests run: 2347 passed, 1 failed
hetzner (full):      Summary [76.3s] 12775/12775 tests run: 12772 passed, 3 failed
```

**CI stopped at test 2348 and never reached the three failures at 6071, 6103 and
9690.** So the containerized leg alone understates the failure set by three, and
the hetzner `--no-fail-fast` run in §2 remains the authoritative picture. Worth
aligning the two invocations; a leg that stops at the first red cannot tell you
how much is red.

### Honest statement of what is NOT done

- The three failures in §2 and the fifth in §5 are **reported, not fixed** — by
  instruction, and because each needs a contract or infrastructure decision
  rather than a lint fix.
- `CI (macos-latest)` and the three remaining `Build` jobs were **still queued**
  when I stopped; macOS runners were the scarce resource all session. The macOS
  clippy leg is unproven in CI, though it shares the Linux code path for all
  four fixes and Linux+Windows are both green locally.
- Run `30396126556` (`7f5c0455`, all four fixes) was **still pending** behind
  `30392102087` in the same concurrency group. Its verdict is not in this
  document.
- I did not merge, open a PR, tag, close an issue, or run `wcore-contract
  generate`.
