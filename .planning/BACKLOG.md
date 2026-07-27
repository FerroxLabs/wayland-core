# BACKLOG

MEDIUM-and-below findings. Per the Phase 20A amended termination rules these are
**logged and explicitly DO NOT BLOCK execution**. CRITICAL and HIGH findings do not
belong here — they must be fixed or disproved, and they live in the owning phase's
baseline or summary.

Each entry names the plan that found it, the evidence, and why it is non-blocking.

---

## From 20A-01 (native Windows/macOS UAT — measure and wire)

### M6 — self-hosted CI contention · MEDIUM · NON-BLOCKING

**Found by:** 20A-01 Task 2 (Wiring A), and pre-registered in the plan's own verification
section.

**What.** 20A-01 added `plan/f20-unified-audit-repair` to `ci.yml`'s `push` trigger so
the branch reaches CI at all. `ci.yml`'s Windows leg runs on
`[self-hosted, Windows, X64, msvc]` — the SAME physical box (`SEANDESKTOP`) that
20A-01 Task 3 measures on and that 20A-02, 20A-03 and 20A-04 all use. The new
`windows-live-acceptance` nightly job added by Wiring C runs there too.

Nothing in this phase serializes them. A CI run triggered by a push can compete with a
measurement in progress for the box, for a cargo target directory, and for any
live-sandbox resource (AppContainer profiles, Job Objects, `%PUBLIC%` test dirs — note
`live_fs_acl.rs` seeds under `%PUBLIC%` and `hard_process_containment_windows.rs`
manipulates real process trees, so two concurrent runs are not merely slow, they can
interfere).

**Why non-blocking.** It degrades measurement *reliability*, not correctness of the
code under test, and it is detectable after the fact.

**Action if it bites.** If a measurement produces an inexplicable result, check
contention FIRST — before diagnosing the code. `gh run list -R FerroxLabs/wayland-core`
for an overlapping run, and re-measure on a quiet box.

---

### F-06 — the Windows box was not pristine when found · LOW · NON-BLOCKING

**Found by:** 20A-01 Task 1, pristine-tree check (REQ-native-r15).

**Evidence.** `C:\ferrox-win` at `ce9a11a6`:

```
$ cmd /c "git status --porcelain --untracked-files=all"
?? crates/wcore-swarm/.swarm-status.json
```

One untracked `wcore-swarm` run artifact, left behind by an earlier measurement. It was
recorded, then removed **by exact path** (never `git clean`, which inside a worktree
deletes branch-committed files), and the tree re-verified empty before anything was
measured.

**Why non-blocking.** It was caught by the pristine check that exists for exactly this
reason, and it was cleaned before any measurement was taken. No result in
`20A-01-BASELINE.md` was measured against a tainted tree.

**Latent issue worth a cheap fix (not done here — out of 20A-01's scope fence).**
`crates/wcore-swarm/.swarm-status.json` is a runtime artifact that is **not**
gitignored, so every `wcore-swarm` run dirties the checkout. That directly threatens the
clean-checkout precondition of `f20-native-windows-proof.ps1`, which hard-throws
`native F20 acceptance requires a clean checkout` on ANY porcelain output including
untracked files. **This is a plausible contributing cause of the checkout-dirty item
routed to 20A-03** — worth checking there before diagnosing anything more exotic.

---

### F-08 — `wayland-e2e-windows-soak.ps1` does not parse under Windows PowerShell 5.1 · LOW · NON-BLOCKING

**Found by:** 20A-01 Task 2, while proving the modified script still parses on real Windows.

**Evidence.** Parsing with `[System.Management.Automation.Language.Parser]::ParseFile`:

| Interpreter | Original script (`2a9d47ff`) | After 20A-01 Wiring B+C (`95552f64`) |
|---|---|---|
| `powershell` (Windows PowerShell 5.1) | **7 errors** | **8 errors** |
| `pwsh` (PowerShell 7) | **0 errors** | **0 errors** |

The 5.1 errors land in code 20A-01 never touched (PHASE H's `cargo mutants --list`, script
lines ~301-302), and the original file already fails identically. Cause: the script uses
non-ASCII characters (`═══` phase banners, `✓`, `✗`, `·` status glyphs) and Windows
PowerShell 5.1 decodes the BOM-less UTF-8 file as ANSI, corrupting string literals. The
extra 8th error is simply the additional `Write-Note` glyphs PHASE L adds — the same
artifact, not a new syntax defect.

**Why non-blocking.** Every consumer invokes `pwsh` (PowerShell 7), where the script
parses cleanly: `nightly-windows-soak.yml` uses `shell: pwsh` and calls
`pwsh scripts/wayland-e2e-windows-soak.ps1`, and the script's own help text documents
`pwsh scripts/...` as the local invocation. The 5.1 failure is unreachable in practice.

**Cheap fix if anyone cares:** save the file as UTF-8 **with** BOM, or replace the glyphs
with ASCII. Not done in 20A-01 — it is neither a defect on the runner that runs it nor
inside this plan's scope fence.

---

### F-11 — three CI jobs cancelled mid-flight, cause not established · LOW · NON-BLOCKING

**Found by:** 20A-01 Task 2 (Wiring A), on the branch's first CI run (`30151510189`).

`CI (macos-latest)` (at step 8), `CI (linux-containerized)` and
`Build (aarch64-pc-windows-msvc)` all concluded **cancelled** while `CI (Array)` concluded
**failure**. Neither obvious explanation fits:

- **Not concurrency `cancel-in-progress`** — `gh run list` for this branch shows exactly
  ONE run and no second push occurred.
- **Not matrix fail-fast** — the `ci` job sets `fail-fast: false`, and `macos-latest` and
  `Array` are both cells of that matrix.

Recorded as an open question rather than guessed at. Re-running the macOS job
(`gh run rerun --job 89662563384`) cleared it and the job reached step 11 normally, so the
cancellation appears transient rather than structural.

**Why non-blocking.** A retry resolved it, and it costs a re-run rather than any code
change.

**Watch for it.** If jobs cancel again on this branch without an explaining push, look at
run-level cancellation and at BACKLOG **M6** (self-hosted contention) together — the
Windows leg shares a box with every downstream measurement.

---

### F-07 — the `c39f7254` / `ce9a11a6` record discrepancy · INFO · RESOLVED, NO ACTION

**Found by:** 20A-01 Task 1.

The plan flagged that `.planning/TEST-AUDIT.md` records the Windows box at `ce9a11a6`
while this session's measurements were labelled `c39f7254`, and instructed the executor
to record what the box actually prints. The box printed `ce9a11a6`. The two records do
not conflict on substance:

```
$ /usr/bin/git merge-base --is-ancestor c39f7254 ce9a11a6                        -> YES
$ /usr/bin/git diff --stat c39f7254 ce9a11a6 -- crates/ .github/ scripts/ justfile .config/
  (empty)
```

Identical code trees. The measurements attributed to `c39f7254` were taken on exactly
the tree the box was standing on. Labelling artefact, not a measurement hazard. No
action; recorded so nobody re-litigates it.

---

## From 21-01 (Phase 21 admission gate + eleven-dimension authority census)

All entries below were surfaced by the 21-01 authority census
(`.planning/phases/21-child-authority-and-budget-inheritance/21-01-AUTHORITY-CENSUS.md`),
measured at SHA `3d80f14662c3df9bd63aeb7ecffc144fe643a553`. The census's four HIGH
findings are NOT here — per the rules they must be fixed or disproved, and they live in
the census and are corpus targets for 21-02.

### MED-1 — the interactive TUI is not drivable on Windows · MEDIUM · NON-BLOCKING

`crates/wcore-eval-scenarios/src/pty_capture.rs` carries `#![cfg(unix)]` at line 63. Its
module header states that `portable_pty`'s Windows ConPTY backend "does not surface the
spawned binary's stdout to the master end in headless CI (the vt100 parser stays empty
and every wait hits its timeout)", with `crates/wcore-cli/tests/harness_tui_flow.rs` as
the in-repo precedent.

**Why non-blocking.** No Phase 21 dimension's *only* live surface is the TUI. The
approval dimension's PTY leg is Linux/macOS-only; its `--json-stream` leg covers Windows.
Recorded here rather than left for 21-03 to discover when the Windows run is due.

### MED-2 — parent/child permit contention on one shared semaphore · MEDIUM · NON-BLOCKING

`active_child_permits` is a single `Semaphore::new(MAX_CONCURRENT_WORKERS)` (=20) carried
into every child spawner by `Arc::clone` (`crates/wcore-agent/src/spawner.rs:2168`). A
parent holds a permit while awaiting its own children, and those children draw from the
same 20. Twenty parents awaiting children can therefore starve the pool.

**Why non-blocking, and why it is explicitly OUT of Phase 21's property.** This is a
liveness/deadlock hazard, not an authority amplification. Phase 21's Success Criterion 1
is about a child *widening* a restriction; sharing the pool is what makes fan-out
non-wideable in the first place. Fixing this must not be done by giving children their
own pool, which would create the amplification the phase exists to prevent.

### MED-3 — `EgressClient::new().with_policy(..)` is a public bypass route · MEDIUM · NON-BLOCKING

`EgressClient::new().with_policy(..)` attaches an explicit per-client policy that consults
neither the process-global `OnceLock` (`wcore_egress::install_global_policy`) nor the
task-scoped policy `AgentBootstrap::build` installs via `with_default_policy`.

**Measured, and this is why it is only MEDIUM.** Every occurrence in the workspace was
checked and **all are inside `#[cfg(test)]` modules** — including the two that look
production at first glance, `crates/wcore-agent/src/spawner.rs:3134` and
`crates/wcore-cli/src/tui/surfaces/mod.rs:4870` (the enclosing `#[cfg(test)]` opens at
`spawner.rs:2962` and `surfaces/mod.rs:3396`). No production site uses it today. It is an
API-shaped hazard, not an open widening route. A lint, `#[doc(hidden)]`, or test-only
gating would close it.

### LOW-1 — `with_reason_state` falls back to rendering the leaf state · LOW · NON-BLOCKING

`crates/wcore-budget/src/execution.rs:641-653` walks leaf-then-ancestors looking for a
state whose `check_state` equals the reason, and falls back to rendering the **leaf** when
none matches — so `limit_for` can report a child's own possibly-wider limit.

**Inert today, and answered by search rather than inference.** All five production
`limit_for` call sites (`cancel.rs:467`, `engine.rs:10841`, `engine.rs:11858`,
`spawner.rs:1180`, `spawner.rs:1204`) render the `BudgetExceeded` payload only *after* the
decision was taken by `first_exceeded_reason()` — directly, or via
`MonitorAction::CancelBudget { reason }` which originates at
`orchestration/monitor.rs:182`. Because the caller has already selected the reason,
`with_reason_state` walks in the same order and finds the same state; the fallback branch
is not taken. **No admission or budgeting decision anywhere reads `limit_for`.** Recorded
so it is not rediscovered as a new finding.

### OOP-1 — `delegate_isolation` F05 identity has not been re-gated · MEDIUM · OUT-OF-PHASE

`.planning/intel/COMPETITIVE-LEDGER.md` assigns Phase 21 the carried limitation "re-run
the F05 capability activation gate against the `delegate_isolation` identity at
`9821ef76` and record the result", because AUTH-* carries an `Unavailable: isolation not
enforced` negative that Phase 20 may already have cleared.

**Why out-of-phase.** That is an F05 capability-gate re-run, not an authority-inheritance
proof. None of the four Phase 21 plans has a task for it and the four-plan cap forbids
adding one. Owner to be reassigned by Sean.

### OOP-2 — `wcore-permissions` has no inheritance model by design · MEDIUM · OUT-OF-PHASE

`crates/wcore-permissions/src/policy.rs`'s own header states the crate's scope is explicit
grants only, with no role hierarchy and no inheritance — directly against F21-01's
"intersection of parent and requested authority" wording.

**Why out-of-phase.** Giving that crate an inheritance model is a design change well beyond
this phase's four plans. Phase 21 proves the property at the seams that actually run (the
budget rollup, the spawn seam, the egress chokepoint, the policy resolver) and records the
permissions crate's shape rather than reshaping it. See census HIGH-3 for the separate,
in-phase question of whether `PolicyGate` is reachable at all.

---

## From 21-02 — dual-surface hostile-child corpus (2026-07-26)

Measured at SHA `4a3dd3756efec29f91fa99ce4a68500c485adc1f` on `hetzner-dsm` and
`SeanD@seandesktop`. Full table and evidence: `21-02-CORPUS-RESULTS.md`. The
three HIGH findings from that run are NOT here — they are 21-03's, per the
amended rule.

### F21-02-04 — live coverage gap: tool and approval · MEDIUM

Any toolset beyond the read-only floor classifies as
`RequestedChildWorkspace::IsolatedMutation`, and durable workspace preparation
refuses in a hermetic non-repository workspace (`durable child workspace
preparation failed: worktree io: orchestrator worktree root must not overlap
repository`). Neither dimension can therefore be observed at the live surface
without a fixture that is a real repository whose worktree root does not overlap
it. Both are recorded NOT-EXPRESSIBLE on at least one live combination rather
than counted as refusals.

### F21-02-05 — live coverage gap: the budget trio · MEDIUM

No shipped surface carries a child-fillable budget field, so a child
budget-widening REQUEST cannot be issued through the product at all; and the
seeded caps tight enough to make the parent envelope bind refuse the parent's own
first turn before any provider call. `time`, `token` and `cost` are
NOT-EXPRESSIBLE on both live combinations. The in-process seam carries the
evidence and the NO-CHANNEL canary carries the future.

### F21-02-06 — egress Ask branch fails open with no doorbell · MEDIUM

With the shipped default config and no consent doorbell attached,
`AgentEgressPolicy::resolve_ask` returns `EgressDecision::Allow`
(`egress/policy.rs:93-97`), so the parent's own policy permits a plain GET to a
non-allowlisted, non-shared-platform host. **Deliberately NOT a Phase 21
widening**: parent and child are equally affected and the child holds the
parent's exact policy object by `Arc` identity, so nothing crosses the authority
boundary. Recorded for triage by whoever owns the egress posture.

### F21-02-07 — the census and plans write a `-p` flag that does not exist · LOW

`wayland-core` has no `-p`. `crates/wcore-cli/src/main.rs:537-539` declares the
prompt as a `trailing_var_arg` positional, so every option must precede it. The
21-01 `LIVESURFACE` rows and the 21-02 plan both write
`wayland-core -p "<prompt>"`. Fix the wording wherever it is reused.

### F21-02-08 — a hermetic live run needs an ephemeral vault · LOW

Under a hermetic `WAYLAND_HOME` with no vault passphrase the binary refuses with
`Session persistence authority unavailable: secure recovery storage is
unavailable`, and every turn fails before reaching a provider. Not a defect — an
environment requirement any future live harness must honour.
`crates/wcore-cli/tests/support/vault.rs` provides both transports (FD for std
children, env for PTY children, because `portable-pty` closes arbitrary
inherited descriptors).

### F21-02-09 — a plan gate is broken for two of its own literals · LOW

The 21-02 Task 2 gate runs `grep -cF "$s"` with `$s='--json-stream'` and
`'--no-tui'`; grep parses the pattern as an option and exits non-zero. The check
needs `-e`. Applies to any future plan reusing that gate shape.

---

## From 21-03 — triage of the corpus reds (2026-07-26)

Logged by `21-03-REPAIR-SET.md`. Non-blocking, per the amended phase rule that
MEDIUM and below go to BACKLOG and do not gate execution.

### F21-02-10 — a pre-existing flaky cancellation test, excluded from the 21-03 repair budget · MEDIUM

**Non-blocking. PRE-EXISTING — not a Phase 21 defect.**
`wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream`
failed all three tries in 21-02's first full aggregate run under corpus load, and
passed in isolation and on the re-run at the recorded SHA.
`.planning/TEST-AUDIT.md:171` already records it as flaky 2/3 and notes that the
`ci` profile's `retries=2` is what turns it green. 21-03 excluded it from its
bounded repair set under the known-red exclusion rule rather than spending the
budget on a red another pass had already triaged. Corpus case:
`21-02-CORPUS-RESULTS.md` `FINDING :: F21-02-10`.

Observation worth keeping for whoever picks it up: the child-authority corpus
adds 22 live binary spawns to the aggregate and plausibly tipped a
timing-sensitive cancellation test rather than exposing a new one.

---

## From 21-04 — the attribution corpus (2026-07-26)

Logged by `21-04-ATTRIBUTION-RESULTS.md` §8. Non-blocking, per the amended phase
rule that MEDIUM and below go to BACKLOG and do not gate execution. The HIGH
findings from the same pass (F21-04-01, F21-04-02, F21-04-03) are NOT here —
they are open to Sean in `21-04-PHASE-VERDICT.md` §4.

### F21-04-04 — `ChildDeliveryTarget::ParentTurn` is unexercised by the attribution corpus · MEDIUM

The journal reducer refuses a declaration carrying `ParentTurn` unless
`parent.turn_id` names a turn the journal has actually seen
(`session_journal/reducer.rs:1597`), and the corpus declares children directly
rather than through a live turn. `SessionOutbox` and `ParentChild { child_id }`
were both exercised, the latter with two sibling grandchildren bound for the
IDENTICAL target — the hardest form, since delivering one must not mark the other
delivered. `ParentTurn` is recorded unexercised rather than faked with a synthetic
turn, which would have proved attribution against a fixture instead of against
the product. Closing this needs a corpus that drives a real turn.

### F21-04-05 — `BudgetTracker::charge` does not block an exhausted session · LOW

`charge` records usage and returns the cap error but does not add the session to
the blocked set; only the admission paths do (`tracker.rs:910/921/932/951` for
`reserve_turn`, `:1038-1083` for `settle_turn`). An escalation therefore cannot
be requested after charging past a cap, only after being refused admission. Not a
defect — `extend_session` gating on `NoExhaustedBudget` is deliberate — but the
asymmetry is undocumented and cost the 21-04 harness one repair iteration. A
doc-comment on `charge` naming the blocking path would close it.

### F21-04-06 — `StreamEnd` is emitted per assistant stream, not per turn · LOW

Any live driver that treats `stream_end` as "the turn is over" will kill the
process while spawned children are still working and then record their absence as
if the topology never existed. 21-04's first instrumented run did exactly that,
and the resulting rows looked like clean negatives. Recorded so the next live
harness does not rediscover it; a note in the json-stream protocol doc would
close it.

### windows-tui — approval and cancellation are unprovable as a human sees them on Windows · MEDIUM

`crates/wcore-eval-scenarios/src/pty_capture.rs` is `#![cfg(unix)]` because
portable_pty's Windows ConPTY backend does not surface the spawned binary's
stdout to the master end, and `crates/wcore-cli/tests/support/pty.rs` inherits the
gate. Every human-visible property is provable on Linux and macOS only. Carried
forward from 21-02's identical limitation at the same severity; it stays open
until the PTY driver supports ConPTY.

### test-isolation — the parallel `wcore-agent` lib run fails a different set of tests every time · MEDIUM

**Three separate agents have independently rediscovered this and spent time on it. Read this
before you do too.**

Raw `cargo test -p wcore-agent --lib` on the 96-core build host fails **a different 13-22 tests
on each run**, at base and on every branch alike. Under `--test-threads=1` the same tree is
clean: measured 2098/0 at base and 2107/0 on a branch — a clean comparison, so nothing about
the failures is branch-specific. A separate agent measured 14 failures parallel vs 2101/0
serial; another measured 2 failures that were `file_watcher_notifier` panicking on EMFILE
("Too many open files") at load 146 with 842 sessions.

Diagnosed causes so far: session-journal writer **lease contention** (`session journal writer
lease is already held`), file-descriptor exhaustion under load, and a `wcore-config` max-tokens
**shared-env race**. `cargo nextest run --profile ci` is unaffected — 3418 passed / 13 skipped /
0 retries consumed on the same tree.

**Why it is worth fixing rather than living with:** the parallel number is the one an unwary
reader quotes. One agent explicitly recorded "a first parallel run said 14 failed" precisely
because 14 is what a careless reader would have reported as a regression. It costs every agent
a diagnosis cycle and it can mask a genuine red.

**Until then:** use `cargo nextest run --profile ci`, or `--test-threads=1` for the raw harness,
and do not report a parallel-run failure count as a regression without the serial control.

### swarm-status-cap-flake — `status_output_cap_kills_git_descendant` flakes ~2/20 · MEDIUM

Pre-existing and unrelated to the F-3 positional-read fix — **it flakes at base too**, confirmed
across matched N=20 full-suite `nextest --profile ci` runs. It drives `assert_clean` through a
fake git script, never creates a transaction root, and races its own 2s capture timeout against
a 4096-byte stdout cap. Left untouched; its timeout was deliberately NOT raised.

For contrast, so this is not confused with the defect that was fixed: F-3 produced **16 flaky
events across 6 tests** at base and **2 events across 1 test** after the fix. This one is the
residue, not the disease.
## F26-01 (lane/26) — out-of-scope findings, non-blocking

- **MEDIUM — pre-existing hermeticity bypass.** `crates/wcore-gateway/src/service.rs:338` calls (was :321 when reported; lane 24b's gateway fix added 83 lines above it and shifted it — the call itself is unchanged and still bypasses)
  `dirs::config_dir()` directly, failing `hermeticity_audit_test::no_dirs_config_dir_bypasses_outside_canonical_helper`.
  Present verbatim at `de977949` (introduced by phase-24 `8b582851`); untouched by phase 26.
  Belongs to the gateway/phase-24 area. Either route it through `wayland_config_dir()` or add it
  to the test's ALLOWLIST with a reason.
- **MEDIUM — `McpServerConfig::headers` is an unfenced future value channel.** Not emitted by
  the portability projection today, so it does not leak; but a future edit that emits it would
  bypass the scrub boundary, and the multi-emitter probe would not notice. Named by the F26-01
  redaction panel. 26-02 should either emit it through `insert_detail` or fence it explicitly.
