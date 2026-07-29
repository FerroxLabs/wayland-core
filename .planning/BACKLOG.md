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

### OOP-1 — `delegate_isolation` F05 identity: ANSWERED, and the answer is a NEGATIVE · MEDIUM · OUT-OF-PHASE

**Updated 2026-07-28. The question this item tracked now has an answer. Phase 20 did NOT
clear the negative — the negative survived it and is emitted by the shipped product.**

**Original item.** `.planning/intel/COMPETITIVE-LEDGER.md` assigned Phase 21 the carried
limitation "re-run the F05 capability activation gate against the `delegate_isolation`
identity at `9821ef76` and record the result", because AUTH-\* carries an
`Unavailable: isolation not enforced` negative that Phase 20 **may** already have cleared.
The 2026-07-26 ledger recorded that Phase 20 "plausibly supersedes" the finding but that
no Phase 20 artifact had re-run the gate, so there was **no evidence either way**.

**There is now evidence, and it runs against the product.** The shipped binary's own
capability-activation stream emits

```
{"type":"capability_activation","capability":"delegate_isolation","stage":"unavailable","reason":"isolation_not_enforced"}
```

**18 times at SHA `2ecdfdf5`** — a commit that is a **descendant of the Phase 20A seal
`9821ef76`** — on host `Ubuntu-2404-noble-amd64-base`, `wayland-core 0.12.25`. Raw
capture: `phases/27-multimodal-browser-generation-voice/evidence/27-01/OBS-RAW.log`.
Corroborated on **Windows** in Phase 25's lifecycle lab:
`phases/25-remote-reach-nodes-plugin-lifecycle/evidence/25-02-win-pos-approved.txt:107`
and `25-02-win-neg-unapproved.txt:98` (2026-07-27; that artifact does not assert its own
SHA). Evidence ID `F05-NEG-PERSISTS@2ecdfdf5`, recorded in
`.planning/intel/COMPETITIVE-LEDGER.md`. **AUTH-\* therefore keeps the negative and stays
at `CONSTRUCTED`, now on measured grounds rather than on the absence of a measurement.**

**Three qualifications, carried verbatim from the ledger so this is not over-read.**

1. This is the **product's own activation stream**, not a fresh execution of the F05
   capability-activation gate harness. It is stronger than absence of evidence and weaker
   than a re-run gate.
2. The observations are at `2ecdfdf5`, which is **earlier than** the Phase 21 authority
   repair at `ac94b1d5`. **Nothing has re-read the identity at or after `ac94b1d5`.**
3. The identity is `delegate_isolation` specifically. It is **not** a statement about
   macOS or Windows sandbox containment generally, both of which `F28-CONTROL-WEDGE` and
   `F-28-02-001-FIXED` measure separately and more favourably.

**What remains open.** Not the original question — that is answered — but its successor:
**re-read the `delegate_isolation` identity at or after `ac94b1d5`**, ideally by executing
the F05 capability-activation gate itself rather than reading the activation stream. Named
as a next-refresh action on the AUTH-\* row of the competitive ledger. Owner unassigned.

**Why still out-of-phase.** It is an F05 capability-gate re-run, not an
authority-inheritance proof. None of the four Phase 21 plans had a task for it and the
four-plan cap forbade adding one. Owner to be reassigned by Sean.

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

### portability-external-paths — the manifest's `external_paths` is an untyped string channel · MEDIUM

Same shape 26-01 closed in `DiscoveredItem.details`, reopened one layer up: an untyped string
field in a document that crosses a trust boundary is where a credential hides. 26-01's case was
an MCP url carrying `?token=`. Type it, or redact it structurally the way the rest of the
portability document already is.

### windows-shell-traps — two cmd/PowerShell forms that silently lose data or exit status · MEDIUM

Both measured on `SeanDesktop` during Phase 26:
- `powershell -File <missing.ps1>; exit $LASTEXITCODE` exits **0**. A gate whose script is
  absent therefore PASSES. Now caught by `lint-plan-gates.py`
  (`powershell-missing-script-exits-zero`), but any *existing* Windows harness predating that
  rule should be re-read.
- `echo X=1>> file` in `cmd` eats the value: `1>>` parses as an fd redirect, so the variable is
  written without it. Quote or space it (`echo X=1 >> file`).

## lane/red-repair — integration-red repair, findings and dispositions

Measured on `hetzner-dsm` at integration base `0f3330e5`. Full-workspace baseline:
`12172 tests run: 12165 passed, 6 failed, 1 timed out, 49 skipped`; workspace build exit 0,
zero errors.

### CLOSED — gateway hermeticity bypass (was filed above by lane/26 as MEDIUM)

`crates/wcore-gateway/src/service.rs:338` no longer calls `dirs::config_dir()`. Fixed in
`lane/red-repair` by routing `SystemdManager::unit_path` through
`wcore_config::config::os_native_config_root()`, the existing sanctioned single-call-site
bypass. The ALLOWLIST was NOT used: the unit file is a *write*, unlike the three read-only
probes already listed, and `wayland_config_dir()` would have broken the feature outright
(systemd's user manager never scans `$WAYLAND_HOME`). See the commit body for the four-way
panel record.

### pipeline-cpu-floor — a fixed per-dispatch CPU cost caps pipeline throughput below ~21/s (debug) · LOW

Measured, not predicted. `AgentEngine::run` costs ~47ms of inline synchronous CPU for ONE
trivial turn against a zero-latency in-memory provider (~6.9ms in release; the rest is debug
overhead). `run_pipeline` drives its per-item futures with `buffer_unordered`, which
multiplexes every future onto ONE task, so that inline CPU cannot overlap.

Scaling of `buffer_unordered` over the spawn path, 60 dispatches each, varying ONLY provider
latency:

| provider latency | width=1 | width=20 | speedup |
|---|---|---|---|
| 0 ms | 48.7 ms/spawn | 50.9 ms/spawn | 0.96x |
| 50 ms | 100.4 ms/spawn | 50.3 ms/spawn | 2.0x |
| 500 ms | 551.1 ms/spawn | 69.1 ms/spawn | 8.0x |

**The initially-suspected defect is RETRACTED.** At zero latency the pipeline's advertised
concurrency of 20 delivers 1x, which looks like a broken engine — but that is an artifact of a
fake provider with no I/O to overlap. Real providers are network calls in the hundreds of ms,
and at 500ms the pipeline streams at 8x and rises toward the configured width as latency grows.
The module doc's claim that items race ahead of one another is TRUE for the production
workload. What remains is a throughput ceiling of roughly `1 / 47ms` per pipeline that binds
only when stage latency falls below the per-dispatch CPU cost. Panel (gemini 3.1-pro, kimi K3,
internal adversarial) unanimous on LOW after the corrected evidence; codex 5.6-sol voted
HIGH/FIX on the pre-correction facts and then dropped its vote on the re-put (no output twice).

Fixing it would mean giving the pipeline task-level parallelism, which needs `'static`
ownership: `WorkflowRunner<'a> { spawner: &'a AgentSpawner }` and `PipelineStageDispatch<'a>`
would become `Arc`-based, changing the public `WorkflowRunner::new(&AgentSpawner)` and its call
sites. Not worth it at LOW, and not a parallel lane's change to make.

### wall-clock-budgeted binary tests are flaky under full-suite load · MEDIUM

Three of the reds/flakes at integration HEAD share one cause: tests that spawn the REAL
`wayland-core` binary under a wall-clock budget of 1-5s, run inside a 12k-test suite at
`test-threads = num-cpus` on a 96-core box that also hosts other lanes' builds.

- `wcore-eval-scenarios::runner_contracts outer_deadline_reaps_owned_descendant_listener` —
  FLAKY (failed try 1, passed try 2). Fails at `runner_contracts.rs:230`, "owned descendant must
  publish pid, port, and heartbeat": `wait_for_orphan_state` polls for ONE second for a freshly
  spawned descendant to write pid/port/heartbeat, against a scenario `max_total_time` of one
  second. Under contention the spawn does not beat the poll window. The product's reaping is not
  implicated — the assertion is about the fixture publishing its own markers.
- `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` — see below.
- `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` — see the
  RED-REPAIR-SUMMARY; not a hang, a 66s debug-build run against a 60s harness budget.

The right repair is per-test, in the harness, by whoever owns test infrastructure: widen the
fixture's publish window relative to the scenario deadline so the two are not racing. A repair
lane forbidden from raising timeouts cannot make that call.
## From lane 24e (Phase 24, F24-04) — 2026-07-27

Each item was found while wiring the typed-client contracts into the ACP request
path. CRITICAL and HIGH were fixed in-lane; everything here is MEDIUM or below
and does not block.

- **[LOW] `acp serve` refuses to start without an LLM provider key** (F24-E-L1),
  even though session create/list/get/delete, the resume route and `initialize`
  need no engine at all, and the server already has an honest "no turn engine
  installed" path for turns. On a host with no provider key the entire ACP
  surface is unavailable rather than degraded. Same SHAPE as F24-D-H1 but with a
  defensible justification, so it is filed rather than fixed: an ACP server that
  cannot run turns is arguably not worth starting. Decide deliberately.

- **[LOW] Live evidence covers ONE event, not an ordered run** (F24-E-L2). The
  engine on the headless Linux host fails fast (no OS keyring, no unlocked
  vault), so a live turn emits a single `error` frame. Delivery-independence is
  fully proved by it — the event was produced 12s AFTER the client had gone and
  was still served on resume — but live multi-event ordering, duplicate and loss
  counting is not. That rests on `typed_client_recovery.rs` (13 events over a
  real severed socket). Close it on a host that can complete a real turn.

- **[MEDIUM] REST `/v1` is gated by role but cannot resume or deduplicate.**
  After F24-E-H1 the REST surface authorizes against the same role table, but
  there is no `/v1` resume route and no `Idempotency-Key` handling on
  `/v1/sessions`. A REST-only client is protected and cannot recover a gap.

- **[MEDIUM] `transport/stdio.rs` and `transport/ws.rs` authorize nothing,
  record nothing and resume nothing.** Neither was touched by this lane.
  **Whether either is reachable in a shipped deployment was NOT investigated** —
  that absence is itself unmeasured and is the first thing to settle, because if
  either is reachable with a verifier installed it is a second F24-E-H1.

- **[MEDIUM] The ACP event log is in-memory and per-process.** A restart loses
  history. The run-scoped stream id makes the loss LOUD (a stale cursor is
  refused by name rather than silently mis-served), which is the contract, but
  no persistence exists.

- **[MEDIUM] Roles bind to ONE principal on the shipped binary.** `acp serve`
  has a single api-key identity, so `--role` sets that identity's role.
  `RolePolicy::grant` supports many principals; multi-principal configuration
  has no CLI surface.

- **[PROCESS, HIGH-severity mechanism, already fixed in-lane] An artifact newer
  than its source is a build that did not happen** (F24-E-P1). `rsync -a`
  preserves mtimes; a tree synced back after a mutation harness had built from a
  MUTATED tree left cargo running the mutant binary — measured, source
  `14:14:55` vs artifact `14:19:15`, two tests reporting a false red. It
  surfaced as a false red here; the same mechanism produces a false GREEN
  whenever the stale binary is the permissive one. Add to the standing
  self-passing-gate list alongside "a test filter that matches nothing exits 0".
  Any cross-host workflow in this programme that syncs with `rsync -a` and then
  runs cargo is exposed.

---

## Phase 29-01 supply-chain census — MEDIUM and below (non-blocking)

Measured at `c6766f02498f7bc7dda1511108c1d59ef9741af0`. Full detail and captured evidence:
`.planning/phases/29-supply-chain-release-integrity/29-01-SUPPLY-CHAIN-CENSUS.md` and
`evidence/29-01/`. The CRITICAL/HIGH set is NOT here — it binds 29-02 through 29-04 and is
listed in the census gap table.

### F29-CEN-21 · shipped `self-update --help` misstates the trust model · MEDIUM

The shipped binary tells the user it "Verifies the `.sig` artifact against the pinned
marketplace pubkey (ed25519) before atomic swap". None of that is true any more: there is no
`.sig` artifact, no pinned pubkey in the update path, and verification is keyless Sigstore via
`gh attestation verify`. `self_update.rs`'s own header records that the advertised scheme was
removed (finding R16) precisely because it shipped an all-zeros placeholder key.

Graded MEDIUM because the actual control is *stronger* than the advertised one and fails closed;
the harm is that a user auditing their own supply chain is told a false mechanism. **Found only
by running the binary** — the string lives in `crates/wcore-cli/src/main.rs:693`, not in the
updater. Not repaired in 29-01 because `main.rs` is the all-lane shared fence. Owner: 29-03.

### F29-CEN-01b · `nightly-windows-soak.yml` bypasses the 1.95.0 toolchain pin · MEDIUM

Three jobs (lines 98, 311, 415) use `dtolnay/rust-toolchain@stable` instead of routing through
`loonghao/vx@v0.9.17`, which honours `rust-toolchain.toml` / `vx.toml`. Not a release path.

### F29-CEN-08 · reproducibility is never measured · MEDIUM

No workflow, recipe or manifest sets `SOURCE_DATE_EPOCH`, rebuilds an artifact, or compares two
build outputs. Each release binary is built exactly once per target, so no variance class is ever
observed. Graded MEDIUM: a detective control, and attestation already binds builder identity.
Owner: 29-02. (A bare grep for `reproducib` hits `ci.yml` five times; all five are comment prose
about a runner crash — recorded as REFUTED, not evidence of a check.)

### F29-CEN-12 · no freshness bound on the update offer · MEDIUM

Zero occurrences of `expires|expiry|timestamp|published_at|created_at|freshness|SystemTime` in
`self_update.rs`; the `Release` struct models only `tag_name` and `assets`, so no publication
time is even parsed. Nothing detects a frozen `releases/latest`. Graded MEDIUM: exploiting it
needs an adversary who can hold a TLS connection to `api.github.com` at a chosen response — a
materially higher bar than the rollback gap (F29-CEN-11, HIGH), which needs no network position.
Owner: 29-03.

### F29-CEN-13 · no revocation surface is consulted · MEDIUM

Zero occurrences of `revoke|revocation|crl|blocklist|denylist` in `self_update.rs`. There is no
path by which a published-then-withdrawn release is refused by an already-installed client.
Owner: 29-03.

### F29-CEN-14 · single compile-time trust anchor · MEDIUM

`pub const RELEASES_REPO` is the sole anchor and there is no runtime override (0 `env::var(`
occurrences — deliberately so; an env var that repoints the updater would itself be an attack
surface). Because the scheme is keyless there is no key list to rotate, but a policy change such
as an org move requires a new binary.

### F29-CEN-07b · `wayland-core-checksums.txt` is unattested · MEDIUM

`sha256sum` runs *after* the attest step and the checksums file is not in `subject-path`, so it
carries no Sigstore attestation while being uploaded as a release asset. The archives are
individually attested, so the file is redundant rather than load-bearing. Owner: 29-02.

### F29-CEN-02b · the release build omits `--locked` · LOW

`release.yml:142,144` build without `--locked`, unlike the seven `--locked` call sites in
`ci.yml` and `justfile`. The committed lockfile is honoured by default, so this is a hardening
gap rather than a demonstrated divergence.
## Phase 28 plan 28-02 — MEDIUM findings, non-blocking

Raised by the E5 certification matrix run. All MEDIUM: each is a real defect that contradicts
none of Phase 28's four Success Criteria, so under the standing severity policy they are
logged here and do not block execution.

- **`F-28-02-003`** — swarm dispatch admission intermittently refuses with the sandbox
  AVAILABLE (`dispatch admission refused: invalid retained workspace reservation`). Observed
  in 1 of 4 clean-lease control observations, and previously twice by another lane, so it is
  not a one-off. Not a sandbox failure. Owner: swarm dispatch admission.
- **`F-28-02-004`** — process finding: the "AppContainer cannot be observed over SSH" rule
  rested on a control that varied the logon while the lease directory was wedged, an
  observation both competing hypotheses predicted. It discounted security evidence for weeks
  across at least four documents. Structural repair already landed in certification contract
  §4.2. Remaining plan-brief copies of the rule need a serialized cross-lane edit.
- **`F-28-02-005`** — a task run through `wayland-core backend run --backend local` on macOS
  created a file outside its workspace (`/tmp`). Whether that is in-profile needs reading
  against the actual sandbox-exec profile; recorded so it is not lost. Owner: `wcore-sandbox`.
- **`F-28-02-006`** — the Linux bwrap backend read-binds ALL of `/etc` (`SYSTEM_RO_DIRS`), so
  a sandboxed worker can read `/etc/shadow`. Measured. This is a deliberate bind and NOT a
  control reporting itself active while inactive — `enforces_read_deny()` means the backend
  honours `fs_read_deny` masks, which it does. Hardening gap. Owner: `wcore-sandbox`.
- **`F-28-01-R001`** — `wayland-core channel` is claimed by a phase 24 artifact and is absent
  from the candidate binary's command tree (`claimed-but-absent`). Surfaced by re-resolving
  the candidate after phase-24 artifacts landed. Needs phase 24's own recorded requirement
  disposition before any cell for that surface could be skipped.
### f23-06-lexical-only-precision — BM25 without a semantic layer mis-ranks concept queries · MEDIUM

Measured through the shipped binary against the whole 3,603-file workspace on Linux AND Windows
(identical, query for query): `wcore-repomap`'s persistent index scores `recall@10 = 1.0000` but
`precision@1 = 0.8125` over the 16-query corpus in `23B-03-LIVE-EVIDENCE.md`. Three
concept-shaped queries put the wrong file first:

| Query | Expected | Actual top hit |
|---|---|---|
| `content hash invalidation` | `crates/wcore-repomap/src/store.rs` | `crates/wcore-agent/src/orchestration/anvil/forge.rs` |
| `worktree identity` | `crates/wcore-repomap/src/scope.rs` | `.planning/phases/20-…/20-06-PLAN.md` |
| `bm25 full text` | `crates/wcore-repomap/src/search.rs` | `crates/wcore-memory/src/retrieve.rs` |

Nothing is lost — every expected file is inside the top 10 — it is ordered wrong, because term
frequency puts a prose-heavy planning document or another crate's doc-comment above the
definition. This is exactly the class the OPTIONAL semantic / dense-vector layer addresses, and
F23-06 marks that layer optional; 23B-03 deferred it under its termination state 2 and recorded
the non-claim. **Do not close this by trimming the corpus to the queries that score well.**

### f23-06-windows-verbatim-prefix-in-scope-fingerprint — `\\?\` leaks into the recorded identity · MEDIUM

On `SeanDesktop` the persistent index's scope fingerprint records
`gitdir=//?/C:/Users/seand/AppData/Local/Temp/…/clone/.git` — `fs::canonicalize`'s verbatim
extended-length prefix, slash-normalised. It is self-consistent (both operands pass through the
same function, and the live branch-switch comparison worked), so nothing is broken today. But a
fingerprint produced by a path that did NOT go through `fs::canonicalize` would not compare equal
to one that did, which would read as spurious scope drift.

### f23-06-exact-fallback-is-a-full-scan — 9x slower than an indexed query · MEDIUM

`wcore-repomap`'s exact-search fallback answers queries FTS5 cannot serve (punctuation-heavy
literals) with `instr()` over the stored text, i.e. a full table scan: **51,601 µs** measured on
`hetzner-dsm` against 5,810 µs p50 for an indexed query. It is bounded by the caller's explicit
limit so it is not a denial-of-service surface, but a caller issuing many such queries pays for
it. A trigram index or a bounded prefilter would close it.

### cargo-hakari-absent-from-hetzner — the workspace-hack gate cannot be run there · LOW

`cargo hakari verify` is in `justfile`'s `check-all` but `cargo-hakari` is not installed on
`hetzner-dsm` and is not a CI step, so no lane can run it on the authoritative build host. 23B-03
added two dependency edges and reported the gate as NOT RUN rather than as a pass, checking the
substantive property directly instead (`git diff -U0 Cargo.lock | grep -E '^[+-]name = '` →
no output, i.e. zero new `[[package]]` entries).

## Phase 29 / 29-02 — dependency-policy findings (MEDIUM and below, non-blocking)

Surfaced by the first-ever execution of `deny.toml` (F29-CEN-04). Full verdict and severities in
`.planning/phases/29-supply-chain-release-integrity/29-02-CLEANROOM-RESULTS.md`;
raw output in `evidence/29-02/deny-verdict.txt`. **`deny.toml` was not weakened to close any
of these**, and `deny` is deliberately NOT chained into `just check-all` while the verdict is red.

- **F29-02-M1 (MEDIUM)** — `RUSTSEC-2026-0192`, `ttf-parser 0.25.1` unmaintained, no safe upgrade
  published. Path: `ttf-parser ← lopdf ← pdf-extract ← wcore-tools`. Upstream suggests `skrifa`.
  Nothing to take at source today; revisit when `lopdf` moves.
- **F29-02-M2 (MEDIUM)** — `RUSTSEC-2024-0370`, `proc-macro-error 1.0.4` unmaintained. Path:
  `proc-macro-error ← utoipa-gen ← utoipa ← wcore-acp`. Build-time only.
- **F29-02-M3 (MEDIUM)** — `RUSTSEC-2025-0141`, `bincode 1.3.3` unmaintained.
- **F29-02-M4 (MEDIUM)** — **Advisory dispositions are fragmented across three files with no
  single source of truth**: `.cargo/audit.toml`, `.github/osv-scanner.toml` and `deny.toml` each
  carry (or fail to carry) their own ignore list. A disposition lifted in one and forgotten in
  the others is silent. This is the structural cause of F29-02-H1.
- **F29-02-L1 (LOW)** — `RUSTSEC-2026-0195` (quick-xml `NsReader` unbounded namespace allocation)
  is **not reachable**: `NsReader` appears zero times in calamine and zero times in the
  workspace. Recorded so the determination is not re-derived from scratch next time.
- **F29-02-L2 (LOW)** — `crates/wcore-fixture-harness/Cargo.toml` declares neither
  `publish = false` nor a licence, so `deny.toml`'s `private = { ignore = true }` does not
  classify it as first-party and it fails the licence gate as `unlicensed`. **One-line remedy:
  add `publish = false`.** Not applied by 29-02: the file is outside that plan's declared
  `files_modified` and its surgical-diff gate, and fixing it alone would not change the verdict
  (advisories still fail) or the `check-all` decision.
- **F29-02-L3 (LOW)** — 60 `duplicate` crate warnings (`windows-sys` ×4, `hashbrown` ×4, `rand`
  ×3 …). Non-failing by policy (`bans.multiple-versions = "warn"`), recorded as the measured
  baseline so a future tightening has a number to compare against.

**HIGH findings are NOT here** — F29-02-H1 and the re-raised F29-CEN-06 are escalations in
`.planning/SEAM-REQUESTS/29.md` (SR-29-6, SR-29-7).

## F29-03-04 — `update_trust.rs` is 1142 lines, over the 1000-line cap (MEDIUM)

Filed by 29-03 at `3658c4281fe228af1f700a56a42e662d3d6a9c7c`. AGENTS.md caps a module at
1000 lines. `crates/wcore-cli/src/update_trust.rs` is 1142 (219 of them doc/comment, 62 the
inline unit-test module). It was NOT split because 29-03-PLAN.md's SURGICAL-DIFF gate
whitelists exactly two `crates/wcore-cli/src` files and a third would have turned red a
scope-control gate protecting five concurrent lanes.

**Remedy:** add `update_trust_wire` to that whitelist and move the wire mirror
(`WireManifest` … `WireTrustedKey`, roughly 200 lines) into
`crates/wcore-cli/src/update_trust_wire.rs`. Non-blocking; the module has one clear purpose
and its inline tests target its own internals, which is where AGENTS.md wants them.

## F29-03-03 — `verify_provenance` is unreachable in production until manifests ship (MEDIUM)

Filed by 29-03. The signed-manifest gate refuses a forward move BEFORE the archive is
downloaded, so `gh attestation verify` is never reached while no release publishes a
`*-release-manifest.json`. The check itself is untouched — 6 call sites, baseline 6 — and
re-enters the path the moment manifests ship. Refusing earlier is strictly safer than
downloading an archive you have already declined to install. Recorded so nobody later reads
the unreached call as a removed one.

## F29-04-01 — `wayland-release manifest build` cannot record a certification binding (MEDIUM)

Filed by 29-04 at `882191d4737c7552bef33214f4aadc31dbf828a3`. Evidence:
`.planning/phases/29-supply-chain-release-integrity/evidence/29-04/run-a1-shipped-tool-only.txt`.

`manifest_build` hardcodes `certification: Evidence::Unavailable` and offers no flag to record
one. `verify_state_chain` refuses release acceptance over an unavailable binding, so the
four-state chain **cannot be completed through the shipped tooling**. Measured with all four
role keys held: every append returned `rc=0`, the acceptance record was minted, and
`state-verify` returned `rc=1` — `release acceptance requires an observed certification
binding`. Holding every key is not sufficient; possession of a signature is not authority,
which is correct.

**Non-blocking** because it is a *tooling completeness* gap on a fail-closed path, not an
unsafe one, and because Success Criterion 4 already grades PARTIAL for an independent and
larger reason (F29-04-03). **Not repaired by 29-04**, which modifies no production source at
all: a corpus written by the hand that fixed the defect proves only that its author knew what
they fixed. Remedy and the requested flag: `SEAM-REQUESTS/29.md` SR-29-14.

## F29-04-02 — release acceptance gates on `Observed`, not on a verified join (MEDIUM)

Filed by 29-04. Evidence: the corpus case `F29-03-BACKEND-RECEIPT-2` in
`evidence/29-04/tamper-corpus-run.txt`.

`verify_state_chain` gates release acceptance on the certification field merely **being**
`Evidence::Observed`. Nothing verifies the binding actually joins to a real receipt, so an
`Observed` binding of invented digests would pass. 29-04's corpus proves the join **can** be
checked — `F29-03-BACKEND-RECEIPT-2` leaves the manifest completely untouched, mutates only
the bound receipt's `identity.binary_sha256`, and the refusal comes from the join itself while
`verify_manifest` still passes. No production path performs that check.

**Non-blocking:** defence-in-depth, not an authorization bypass. The manifest must still be
signed by the release-acceptance key, and that key holder is the authority the binding exists
to give machine-checkable evidence *alongside*, not to constrain. Related to **R28-A** —
until the receipt binds the distributed archive to the certified binary, a stronger join has
less to verify. Remedy: SR-29-14 item 2.

## F29-04-03 — the four-state ledger is not wired into `release.yml` (MEDIUM)

Filed by 29-04. Evidence: `evidence/29-04/release-pipeline-states.txt` (re-measured at this
commit, not inherited from 29-01's census).

Three greps: zero `environment:` declarations in **any** workflow (the only native
manual-approval gate); zero mentions of rollback in **any** workflow; zero occurrences of
`wayland-release`, `state-append`, `state-verify` or `release-manifest` in `release.yml`. One
tag push matching `v*-wayland-*` drives build → github-release → publish-npm. Packaging,
deployment preparation and release acceptance are **one authorization act** in the product.

**Non-blocking as a finding, but it is the finding that holds Success Criterion 4 at PARTIAL**
and the verdict says so explicitly. It is filed here as the tracking entry; the actionable
request, with what to add and where, is `SEAM-REQUESTS/29.md` SR-29-13, owned by release
coordination. `.github/workflows/` is fenced out of every Phase 29 plan.

## F29-04-04 — CORRECTION: `seandesktop` is reachable as `SeanD` (MEDIUM)

Filed by 29-04. 29-03's `evidence/29-03/REAL-KEY-LIMITS.tsv` records `F29-LIMIT-06` as a
REAL-ACCOUNT blocker — "SSH access to the `seandesktop` host, which is refusing authentication
on every account tried". Falsified from the Mac at this commit:
`ssh -o BatchMode=yes SeanD@seandesktop 'hostname'` → `SeanDesktop`, `rc=0`. The account is
**`SeanD`**.

**29-04 did not run the Windows leg** — out of scope, and a concurrent lane holds that host.
`seandesktop` is one physical machine and contention there corrupts proofs. **Serialize:**
whoever picks this up waits for the windows-requeue lane and states which quiet run each
figure came from. Filed so a wrongly-reported blocker does not cost a Sean-reserved round trip
for nothing. Full note: SR-29-15.

**HIGH findings are NOT here.** Phase 29 closes with **two open HIGHs**, both escalated in
`.planning/SEAM-REQUESTS/29.md` and neither closable inside the phase: **F29-02-H1** (the
`.cargo/audit.toml` "sole path" suppression, amended by 29-04 — SR-29-6) and **F29-03-01**
(`self-update` installs nothing until a real trust root and a published manifest asset exist —
SR-29-9 / SR-29-11). Their effect on the grades is stated in `29-PHASE-VERDICT.md` rather than
absorbed.

---

## CLASS-ENV-01 — process-wide env mutation in parallel tests, three independent sightings (MEDIUM, non-blocking)

Three lanes found this from three different directions on 2026-07-28 without knowing about
each other. It is one root cause, not three flakes, and it manufactures **false reds** — which
costs more than it looks like, because a false red under contention is indistinguishable from
a regression until someone spends a session proving it isn't.

| Sighting | Where | What was measured |
|---|---|---|
| lane/25-cloud | `crates/wcore-agent/src/.../registry.rs` | `a_recorded_task_is_readable…` flakes under bare `cargo test`. Attributed by measurement, not assumption: same worktree, same commit, only `cloud.rs` swapped — **merge-base 6/12 failed**, lane 8/12, `--test-threads=1` **84/84 clean**. Pre-existing, untouched by the lane. |
| lane/core-254 | `crates/wcore-config/src/website_policy.rs:683` | The comment claims `#[serial_test::serial]` "serializes every env-mutating test in this binary". **It does not** — the three readers are plain `#[test]`. Base full suite green 932/0, head full suite red 932/3, same 3 green in isolation at both. The PR **exposes** it, does not cause it. Filed there as CR-6. |
| orchestrator | `wcore-skills` watcher tests | Different mechanism, same lesson: `fs.inotify.max_user_instances` at 128 with ~109 held by six lanes → ~20 EMFILE failures at 0.007s. Raised to 512 at runtime (**resets on reboot**). Same suite passed **669/669 in isolation at the identical commit.** |

**The standing rule this justifies, which is already costing sessions when ignored:** a
full-workspace run taken while other lanes are building **is not a measurement**. Re-run the
crate alone at the same commit before reporting any regression, and state which run each figure
came from.

**Why it stays MEDIUM.** It is test-infrastructure, not product behaviour; `cargo nextest run
--profile ci` is unaffected (3418 passed / 13 skipped). It does not block a release. But it
should be fixed once rather than re-diagnosed per lane — the fix is to stop mutating
process-wide env in tests (per-test config injection), not to add more `#[serial]` attributes,
which is what the `website_policy.rs` comment shows people reaching for and what demonstrably
did not hold.

**Do not "fix" this by raising a timeout, adding `#[ignore]`, or serializing the whole suite to
green.** A reported red is worth more than an engineered green; the goal is tests that do not
share hidden global state.

---

## CLASS-CONTRACT-01 — the contract corpus reddens every lane by construction (MEDIUM, friction not defect)

`crates/wcore-protocol/src/contract/spec.rs:833` — `SOURCE_INPUTS` digests **40 engine source
files**, among them `crates/wcore-cli/src/main.rs`. That is precisely the file `LANE-BRIEF` §6
instructs **every** lane to edit (the shared fence). The consequence, measured by
`lane/false-advertising` and verified green-at-base then red-at-first-commit before being
attributed:

> **Every lane goes red on `desktop_contract_corpus` without changing any wire shape.**

`schema_digest` stays **UNCHANGED** in this situation; only `fixture_digest` and
`source_inputs_digest` move — the same benign shape Sean authorized regenerating at `c743f398`.

**Handling, which is orchestrator-level and must not be devolved to lanes:** ONE regeneration
over the *merged* tree clears all lanes at once. Per-lane regeneration is actively harmful —
the artifact is byte-exact, so concurrent regenerations conflict, and each goes stale the
moment another lane merges. Lanes are correctly told **not** to run `wcore-contract generate`.

**Desktop must re-pin in the same release train.** `observation.rs:329` makes a digest mismatch
a HARD ERROR at `ready` negotiation, so an un-re-pinned Desktop will not connect.

**Why this is friction rather than a defect:** the digest is doing its job — it is a tripwire
over engine inputs, and it fires. The design question worth revisiting later is whether
`main.rs` belongs in `SOURCE_INPUTS` at all, given it is a CLI registration surface rather than
a wire-shape input. Narrowing that set would remove the recurring cost without weakening the
tripwire over the files that actually determine the contract. **Do not narrow it reactively to
make a red go away** — that is the tripwire-weakening move, and it must be a deliberate,
evidenced decision about what determines the wire shape.

## F26-04-B — a case-only or normal-form-only peer name collapses BEFORE the product sees it (MEDIUM)

Filed by 26-04, and measured on Sean's own hardware on 2026-07-28 rather than assumed.
The hostile corpus generator verifies AFTER creation that a declared name distinction
actually survived on the filesystem it just wrote to:

| distinction | Linux (hetzner-dsm) | Windows (SeanDesktop, NTFS) | macOS (Sean's Mac, APFS) |
|---|---|---|---|
| two peer profiles differing only by CASE | distinct | **collapsed** | **collapsed** |
| two peer profiles differing only by Unicode NORMAL FORM | distinct | distinct | **collapsed** |

The middle column is the non-obvious result: **NTFS is case-insensitive but NOT
normalisation-insensitive**, so a single "Windows and macOS both collapse" assumption is
wrong in one of the two directions. The consequence for the product is that on macOS an
operator migrating two case-distinct peer profiles loses one AT THE SOURCE, before
`migrate` is ever invoked — and the product cannot report what it was never shown. Nothing
in Core can fix this; the most it can do is DETECT and warn, which it does not today.

macOS is not a gate host in this phase, so this is recorded rather than closed here.
**Phase 28's native certification owns it.**

## F26-04-C — the directory-level symlink escape produces no NAMED refusal (LOW)

Filed by 26-04. The hostile case `escape-symlink-dir` replaces an entire skill directory
with a symlink out of the source root. The walk simply does not descend it, so the product
emits no refusal naming that item — unlike the file-level escapes, which produce
`refused: imported executable content contains a symlink: <path>`. The case's evidence is
therefore the external sentinel digest (unchanged, both platforms) and the item's absence
from the quarantine store, which is weaker than a named refusal. Recorded so nobody later
mistakes that case for stronger evidence than it is. Not a defect: nothing escaped.

## F26-02-C — RE-CONFIRMED by 26-04, still open (MEDIUM)

`scripts/panel-decision-check.sh` REJECTS a capture carrying a repeated IDENTICAL
`PANEL-VERDICT` line, which is a shape codex measurably emits. 26-02 escalated this to
26-01 rather than editing a file outside its declared set; 26-04 is under the same
direction and does the same. Measured again at 26-04's own panel record, each shape in
isolation under POSIX `sh`:

| capture shape | required | measured |
|---|---|---|
| kimi bullet-prefixed and indented verdict | ACCEPT | **rc=0, accepted** |
| codex repeating its final block (identical verdict twice) | ACCEPT | **rc=1, REJECTED** |
| two DIFFERENT verdict values in one capture | REJECT | rc=1, rejected |

**No vote was lost in 26-04's run** — measured: the real codex capture carries exactly
**1** `PANEL-VERDICT` line, and the panel record passes the checker (`PANEL RECORD OK`).
The gap is that the harness would drop a vote IF codex emitted its duplicate shape. The
fix belongs in `panel-decision-check.sh`'s `verdict_count`, which should tolerate N
identical verdict values and reject only N distinct ones — the extraction is already
correctly unanchored, so only the count is wrong.

## CLASS-WIN-LIVE-01 — `live_integrity` job-reaping/timeout cases are red at base on Windows (MEDIUM, pre-existing)

**Found by:** `lane/254-take` while measuring the #254 cwd fix. **Not caused by that lane** —
established by a base-vs-head comparison, not by argument.

With `WAYLAND_SANDBOX_LIVE_WINDOWS=1` set, `cargo test -p wcore-sandbox --test live_integrity`
on `SeanD@seandesktop` fails the same two cases at the merge-base as at the lane head:

| Commit | Worktree | Result |
|---|---|---|
| `14905684` (merge-base, no lane changes) | `C:\ferrox-254-base` | **3 passed; 2 failed** |
| `db391a0a` (lane head) | `C:\ferrox-254-take` | **3 passed; 2 failed** |

Same two cases both times: `live_future_drop_reaps_descendant_job_tree` (line 287) and
`live_runaway_command_is_bounded_by_timeout` (line 210). An earlier head run reported
**0 passed / 5 failed in 25.72s** where a later run at the identical commit reported
**3 passed / 2 failed in 16.12s**, so the case set is also non-deterministic — consistent with
the known wall-clock-budget-under-load class rather than a logic defect.

Both are wall-clock-budgeted (a 10s `tokio::time::timeout` around a heartbeat poll, and a
timeout-bound runaway). They are NOT `#[ignore]`d, so they run in a default
`cargo test -p wcore-sandbox` — but only do real work when `WAYLAND_SANDBOX_LIVE_WINDOWS=1`.

**Do not fix by raising the timeout or adding `#[ignore]`.** The budgets are the assertion.

## CLASS-WIN-LONGPATH-01 — `atomic_write` and Win32 long-path handling (MEDIUM, NOT investigated)

Recorded per the `lane/254-take` brief, which flagged that `atomic_write` reaches Win32 without
`std::fs`'s long-path handling and that ~41 modules call it.

**Stated honestly: this lane did NOT investigate or reproduce it.** It is adjacent in theme to
the cwd fix (both concern the `\\?\` verbatim prefix at the Win32 boundary) but it is a
different mechanism and the opposite direction: the cwd fix *strips* a verbatim prefix so
`cmd.exe` will accept a current directory, whereas long-path support requires *adding* one so
a >MAX_PATH path can be opened. Fixing one neither causes nor cures the other. Needs its own
investigation before anyone acts on it — do not treat this entry as a confirmed defect.

---

## From lane/26-gaps — the two gaps Phase 26 left open (2026-07-28)

### F26-GAPS-01 — `config.toml` profile ORDER is nondeterministic, and it flakes a shipped test · MEDIUM

`Config::profiles` is a `HashMap<String, ProfileConfig>` (`crates/wcore-config/src/config.rs:319`)
and Rust's default hasher is randomly seeded per process, so two IDENTICAL clean runs of
`migrate hermes` against one corpus emit the same profile sections in different orders.
Measured directly on the base binary at `c23a08b9`: two runs, same corpus, same set of
profiles, `[profiles.prof02]` in a different position, every other byte equal.

This is not cosmetic in one respect — it makes a shipped test a coin flip.
`crates/wcore-cli/tests/migrate_hermes.rs:287` (`import_is_idempotent_without_overwrite`)
asserts the re-imported `config.toml` is byte-equal, and **fails 13 of 20 runs** measured
at `a170ee24` on `hetzner-dsm`. The assertion is correct; the product's output is not
stable. It predates this lane: the ordering was measured on the base binary before any
change here, and the only product change this lane makes is to
`QuarantineStore::save_index`, which writes `migrate-quarantine/index.json` and never
touches `config.toml`.

**Prescribed fix, deliberately NOT taken here:** `HashMap` → `BTreeMap` for
`Config::profiles` (and, on the same grounds, `providers`). That is a public shared-type
change in `wcore-config`, which this project's own standing lesson requires verifying with
`cargo check --locked --workspace --all-targets` rather than a per-crate check, because a
per-crate check misses downstream exhaustive matches. Four lanes were building concurrently
when this was found, and a shared-schema change taken mid-flight is exactly the seam the
Phase 26 certification records as deliberately uncrossed. MEDIUM, so non-blocking by the
standing severity policy.

### F26-GAPS-02 — an interrupted `migrate` leaves ORPHAN payload directories · LOW

Separate from `F26-GAPS-H1` (fixed in this lane). `apply_plan` writes each quarantined
payload with `write_tree` and only then saves the index, so a kill in that gap leaves a
payload directory on disk that no index entry claims. Measured across 35 mid-apply
interruptions at `a170ee24`: 20 trials carried between 1 and 379 orphan directories.

Every one of them recovered when the product was driven again, because `write_tree` merges
over the existing directory with identical bytes for an identical corpus, and the re-drive
re-admits the item. **The residual risk is narrower and was NOT measured here:**
`write_tree` does not clear the destination first, so if the SOURCE item changed between
the interrupted run and the re-drive, a stale file from the first attempt can survive
alongside the new ones while the index records the NEW digest. Recorded as the unmeasured
observation it is, with its code reference, rather than as a finding this lane proved.

### F26-GAPS-03 — F26-03's first clause is genuinely unimplemented, and now has its facts · MEDIUM

See `26-GAPS-SUMMARY.md`. The disposition is recorded so the clause cannot reach another
phase unnoticed the way it reached this one.

---

## From 28-04 — every ACCEPTED and DEFERRED Phase 28 finding (2026-07-28)

Written **before** the finding ledger cited these ids, so each id is real rather than
aspirational, and gate-checked by
`python3 .planning/scripts/f28-ledger.py --check-backlog-ids <findings.tsv> .planning/BACKLOG.md`.

Every entry below is also enumerated **inside** the signed certification receipt
(`28-04-CERTIFICATION-RECEIPT.json`, `body.findings`). That enumeration is the entire
consideration the program receives in exchange for not fixing these, and it is machine-readable
on purpose: **if Phase 29 does not read it, this accounting control has no consumer and the
Phase 28 acceptance rule is worth less than it looks.** That dependency is stated rather than
assumed — it is the open risk the acceptance decision itself carried forward.

Nothing here is CRITICAL or HIGH. The one Phase 28 finding at HIGH that could be neither fixed
nor disproved (`F-28-02-002`) is **not** in this file: it is OPEN, it blocks the acceptance
gate, and it lives in `28-04-FINDING-LEDGER.md` and the receipt.

| Backlog id | Findings | Severity | Owner |
|---|---|---|---|
| `BL-F28-KR02` | KR-02 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-KR03` | KR-03 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-KR04` | KR-04 | LOW | Phase 29 (release acceptance) |
| `BL-F28-C4` | F-28-01-001, F-28-01-003 | MEDIUM | Phase 29 / Sean |
| `BL-F28-POLICY-DOC` | F-28-01-002 | LOW | Phase 29 (release acceptance) |
| `BL-F28-SURFACE-UNCLAIMED` | F-28-01-R001/R002/R004/R007/R008/R010/R012/R014/R017 | MEDIUM ×9 | Phase 29 (release acceptance) |
| `BL-F28-SURFACE-WEAK` | F-28-01-R003/R005/R006/R009/R011/R013/R015/R016/R018 | LOW ×9 | Phase 29 (release acceptance) |
| `BL-F28-SWARM-ADMIT` | F-28-02-003 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-BELIEF` | F-28-02-004 | MEDIUM | orchestrator |
| `BL-F28-LOCAL-BACKEND` | F-28-02-005, F-MA-002 | MEDIUM, LOW | Phase 30 (hardening) |
| `BL-F28-BWRAP-ETC` | F-28-02-006 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-SYSTEM-DACL` | F-WR-05 | LOW | Phase 30 (hardening) |
| `BL-F28-WIN-PARALLEL` | F-KR-08 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-ACL-COST` | F-KR-09 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-BENCH-SANDBOX` | F-28C-01 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-ACP-HERMETIC` | F-28C-02 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-HEADLESS-KEYRING` | F-28C-03 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-TEMP-SCRATCH` | F-MA-001 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-WEDGE-BASHPATH` | F-28-04-002 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-FLAVOUR-D` | F-28-04-003 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-TWO-CANDIDATES` | F-28-04-004 | MEDIUM | Phase 29 (release acceptance) |
| `BL-F28-MACOS-CENSUS` | F-28-04-005 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-SOAK-WORKLOAD` | F-28-04-006 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-RUNLEVEL-ACTIVENESS` | F-28-04-007 | MEDIUM | Phase 30 (hardening) |
| `BL-F28-CONTRACT-CORPUS` | F-28-04-008 | MEDIUM | Sean / Desktop release coordination |

### `BL-F28-KR02` — Windows snapshot private DACL enforcement is unproven (MEDIUM)

`snapshot.rs::windows_private_dacl_accepts_restrictive_deny_ace` and `..._rejects_null_empty_and_broad_allow`
fail at their `WRITE_DAC` reopen step with error 5, **identically at parent**, so this is not a
candidate regression. The 28-01 contract's §3.4 "unproven-control corollary" would have moved
this across the A2 line and was deliberately not applied.

### `BL-F28-KR03` — worker output-exhaustion buffer-retention bound is unproven (MEDIUM)

`worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers` takes
~35 s against a 20 s budget. **The timeout was deliberately NOT raised.** The red is a budget
overrun in the test, not an observed retention of buffers.

### `BL-F28-KR04` — bash cannot run under Windows AppContainer, by construction (LOW)

msys needs `\BaseNamedObjects`; AppContainer confines to `AppContainerNamedObjects` (`0xC0000022`).
No budget fixes this. The product contract is fail-closed and the test asserts it. This is the
canonical `architectural-impossibility` instance and its impossibility check **is** that
fail-closed assertion.

### `BL-F28-C4` — the acceptance rule's own counter-evidence (MEDIUM)

Two rows. **`F-28-01-001`:** the unproven-control corollary of A2 was considered and deliberately
not applied, so `KR-02` and `KR-03` stayed below the A2 line — recorded so a later reader applies
it deliberately rather than rediscovers it. **`F-28-01-003`:** the severity-amendment commit
`d0837aa7`, whose being the "later instrument" is the whole load-bearing structure of the
`c4-disposition` decision, ends its own message with *"Phase 28's criteria are untouched
(different phase)."* That is the strongest available evidence for the losing `c4-literal`
position and it appears in no captured panel response. **Anyone reopening the Phase 28
acceptance rule should start here.**

### `BL-F28-POLICY-DOC` — the standing severity policy is not in `AGENTS.md` (LOW)

Plans cite it as living there; it does not. Either add it to `AGENTS.md` §11 or correct the plans.

### `BL-F28-SURFACE-UNCLAIMED` — 9 shipped surfaces claimed by no phase artifact (MEDIUM ×9)

`agent`, `crucible`, `forge`, `init`, `mcp-serve`, `models`, `project-context`, `self-update`,
`swarm`. All were exercised by the matrix and the soak, so coverage is unaffected — what is
recorded is that **attribution** is incomplete. They may predate phases 24–27, which is itself
worth knowing at certification time. **The `F-28-01-R0nn` ids are POSITIONAL and shifted between
the 28-02 and 28-03 resolutions; do not diff them by id.**

### `BL-F28-SURFACE-WEAK` — 9 surfaces the resolver cannot attribute confidently (LOW ×9)

`fetch`, `goal`, `image`, `migrate`, `profile`, `sandbox`, `session`, `setup`, `workflow`. A limit
of the **instrument**, deliberately never rendered as a fact about the product.

### `BL-F28-SWARM-ADMIT` — swarm dispatch admission intermittently refuses with the sandbox available (MEDIUM)

Measured by the 28-02 control, not inferred: `obs-scheduled-task-cleared` shows
`probe_report=available` with `product_behaviour=refused-fail-closed`. Fails **closed**, so it
costs availability rather than containment.

### `BL-F28-BELIEF` — an unmeasured belief suppressed real security evidence for weeks (MEDIUM)

The standing rule *"never conclude a red from an SSH run"* had no discriminating control behind
it and discounted Windows sandbox reds. **CONFIRMED-FALSE and retracted** at 28-02; corrected in
`LANE-BRIEF` §2 and `AGENTS.md` §11. Not marked FIXED because remaining plan-brief copies still
carry it and need a serialized cross-lane edit by the orchestrator.

### `BL-F28-LOCAL-BACKEND` — `backend run --backend local` names containment it never applies (MEDIUM + LOW)

The module's own doc says it *"CONSULTS `wcore_sandbox::default_for_platform()` … but does NOT
currently route the child through `SandboxBackend::execute`."* Its receipt honestly reports
containment *"selected but NOT applied to this child"*, which is why `F-MA-002` is LOW rather
than an instance of the `KR-05` pattern. This is the path that produced `F-28-02-005` (a task
created a file outside its workspace), so that measurement evidences nothing about `sandbox-exec`.

### `BL-F28-BWRAP-ETC` — the Linux bwrap backend read-binds all of `/etc` (MEDIUM)

A sandboxed worker reads `/etc/shadow` (`F28_SHADOW=READ`, reproduced twice independently).
Source read before scoring: `SYSTEM_RO_DIRS` includes `/etc` and the bind is a deliberate
`--ro-bind /etc /etc`, so `enforces_read_deny() == true` is **not** lying — it means the backend
honours `fs_read_deny` masks, not that it denies everything ungranted. A hardening gap, **not** a
control that reports itself active while inactive.

### `BL-F28-SYSTEM-DACL` — running the sandbox as SYSTEM trips `validate_mutex_security` (LOW)

`acquire()` always builds a 2-entry DACL while the validator expects 1 when the caller's SID *is*
the SYSTEM SID. Fails closed with a message that reads like a platform limitation — the `KR-05`
pattern again — but SYSTEM is not the shipping configuration.

### `BL-F28-WIN-PARALLEL` — concurrent live AppContainer executions interfere (MEDIUM)

3/2, 2/3, 1/4 in parallel versus a flat 4/1 across 12 serial runs. The observed failure is the
product's fail-closed guard **declining to measure an ambiguous scope**, which is correct
behaviour. **Operating consequence: `--test-threads=1` is a CORRECTNESS REQUIREMENT for live
sandbox suites, not a preference, and any live-Windows figure this program recorded from a
parallel run is untrustworthy.** This is the cause of `CLASS-WIN-LIVE-01`.

### `BL-F28-ACL-COST` — AppContainer ACL grant+revoke is O(objects), paid every execution (MEDIUM)

133 ms at 200 objects, ~10 s at `%TEMP%`'s 57,636, 19,487 ms at 200,000. The field allowlist the
repaired test itself documents (`~/.cache`, `~/.cargo`, `~/.npm`, `~/.rustup`) is exactly the
large-tree case, so a real user can pay tens of seconds of setup **on every sandboxed command**.
A cost defect, not a containment defect.

### `BL-F28-BENCH-SANDBOX` — `tool_token_bench` cannot measure `BashTool` on any host (MEDIUM)

It dispatches through a context with no sandbox backend, so every Bash row fails closed and the
sanity gate refuses to write the markdown. **The bench's Bash column has never been produced.**

### `BL-F28-ACP-HERMETIC` — `acp_engine_turn` is non-hermetic while documenting itself as hermetic (MEDIUM)

It calls `Config::resolve()`, which reads the operator's real `~/.config/wayland-core/config.toml`,
so its result depends on the developer's machine. Attributed decisively by removing the host
config and watching the panic **move line**.

### `BL-F28-HEADLESS-KEYRING` — no ACP/A2A session on a headless Linux host with no keyring (MEDIUM)

Unless the operator sets `credentials.backend = "encrypted-file"` and supplies a passphrase.
Fail-closed and actionable with two documented remediations, so not a security defect — but
headless Linux is the canonical deployment for an agent CLI and this is a first-run wall.

### `BL-F28-TEMP-SCRATCH` — `WorkspacePolicy::contained` grants the whole host temp dir (MEDIUM)

`scratch_dirs()` grants all of `std::env::temp_dir()`, so a contained shell may write anywhere
under `/tmp`. Deliberate and documented in code. **Measured rather than argued:** it is what
produced the first red of the new e2e containment gate, whose escape target sat in a
`tempfile::tempdir()` the policy legitimately grants.

### `BL-F28-WEDGE-BASHPATH` — the KR-05 residual (MEDIUM)

Whether the **bash-tool** path (`default_for_platform()`) executes unsandboxed under a wedged
AppContainer lease is unmeasured; the 28-02 control exercised `SandboxRegistry::required_for_session`.
Named explicitly rather than absorbed into `KR-05`'s DISPROVED row, because 28-02 said in terms
that KR-05 must not be closed on the delegated surface alone. **What would close it:** drive
`wayland-core sandbox exec` on `seandesktop` with a lease deliberately wedged and observe
whether the child carries a containment signature or an uncontained High-integrity label.

### `BL-F28-FLAVOUR-D` — zero-execution flavour (d) survives for plain `cargo test` (MEDIUM)

19 feature-gated and 25 platform-gated test binaries print `running 0 tests` / `ok` and exit 0;
the largest blanks 16 tests. The invocation-site fix (`no-tests = "fail"`) closes it for
**nextest only**, and that limit is stated rather than glossed.

### `BL-F28-TWO-CANDIDATES` — the certification spans two candidates (MEDIUM)

Linux and Windows matrix legs at `32e2f57d`; macOS matrix re-run and all three soak legs at
`e4a3f5fc`. **No single-candidate full matrix exists for Phase 28.** Eleven merges landed between
them, adding 15 surfaces including the `sandbox` verb that makes the macOS re-run possible at all.
The receipt binds each candidate exactly and per-scope rather than picking one and calling it
"the" candidate, so the binding is honest and the coverage is what is split. **Phase 29 should
decide whether a single-candidate full matrix is a release prerequisite.**

### `BL-F28-MACOS-CENSUS` — the macOS orphan census is non-authoritative (MEDIUM)

It observes a process group, and a hostile descendant can leave one, so its zero is a zero
**observation** rather than a containment guarantee. The instrument is weaker, not absent: a
deliberately orphaned **product** process was planted and **found**. Linux (cgroup-v2) and
Windows (job object) are authoritative.

### `BL-F28-SOAK-WORKLOAD` — the soak workload is read-only, so `state_dir_bytes` is flat (MEDIUM)

301 bytes at the first sample and 301 at the last, on every family. A true measurement and a weak
one: a green means *"a thousand read-only sessions wrote nothing"*, **not** *"the product does not
accumulate state under use"*. A state-accumulating workload is a deliberate future choice.

### `BL-F28-RUNLEVEL-ACTIVENESS` — the macOS activeness observation is run-level (MEDIUM)

One containment differential applied to all 24 macOS `sandbox-probes` cells rather than one
observation per cell. Raised by 28-02, carried by 28-03, still true. Not resolved here because
resolving it means re-running a measurement (forbidden by 28-04's scope fence) and narrowing the
cell set would be a silent reduction of coverage.

### `BL-F28-CONTRACT-CORPUS` — `desktop_contract_corpus` red, run by no Phase 28 lane (MEDIUM)

`CLASS-CONTRACT-01`, structural. Closing it would require `wcore-contract generate`, a
release-coordination action explicitly reserved and **not** performed by this phase. Recorded for
completeness because three separate lanes named it and declined it.

### `BL-F28-VACUOUS-GREENS` — four of five `actor_acl_test` tests cannot fail (MEDIUM)

All four assert *"the tool runs"*, which is trivially true when the deny pre-filter they exist
to test does not exist in production. The asserted enforcement string
(`"Denied by sub-agent learned policy"`) occurs in **exactly one file in the workspace — the
test itself**; `CallActor::SubAgent` has no production construction site; every production site
sets `learned_policy: None`. Same class as an all-ignored suite, one layer deeper: not zero
tests executed, but four executed tests that cannot fail. **Read the suite as a forward spec,
not a certification input.**

### `BL-F28-COUNT-INFLATION` — `acp_engine_turn` reports `8 passed` while running neither named case (MEDIUM)

`#[path = "support/mod.rs"] mod support;` compiles 8 further non-ignored tests into the binary,
so `cargo test --test acp_engine_turn` prints `8 passed` and exits 0 having executed **neither**
of the two cases the binary is named for. This defeats the program's own counter-rule — *"read
the `N passed` count back"* sees a healthy 8 and is satisfied. A guard worded for this specific
hazard was added, but it was produced by the same generator as nine others and its falsification
was measured for three suites rather than for this binary individually, and the count inflation
itself is unchanged. **Not claimed FIXED.**

### `BL-F28-MACOS-INSTRUMENTS` — the macOS matrix members use no containment differential (LOW)

`hard_process_containment_macos` infers reaping from a **wall-clock bound** (`sleep 45 &` under
sandbox-exec must return in < 20 s, so a non-reaping backend holding the stdout pipe cannot
pass) and `live_integrity_macos` from a **matched write pair** (inside the allowlist succeeds
and lands on disk; outside is denied and never reaches the filesystem). Both are substantive and
the bound is **one-sided in the safe direction** — runner load can only produce a false FAIL.
The residual: neither forms a containment *differential*, so macOS evidence for Criterion 1
rests on two different instruments rather than one.

---

## From lane 29-deny (dependency policy — `cargo deny`)

### `BL-F29-DENY-UNMAINTAINED` — five unmaintained transitives held by exception · MEDIUM · NON-BLOCKING

**Found by:** lane 29-deny, taking `cargo deny check` from exit 5 to exit 0.

**What.** `deny.toml` now carries five `[advisories] ignore` entries. Every one is an
`informational = "unmaintained"` advisory with `patched = []` in its RustSec source — meaning
**no version of the flagged crate clears it**, so a bump is not a fix and only removal is. All
five were checked for reducibility against crates.io metadata, and none is reducible from here:

| advisory | crate | root chain | why it stays |
|---|---|---|---|
| RUSTSEC-2025-0141 | bincode 1.3.3 | `<- syntect 5.3.0 <- wcore-cli` | syntect 5.3.0 is the LATEST published syntect. Dropping its `dump-load` feature would remove bincode and also delete TUI syntax highlighting. |
| RUSTSEC-2026-0192 | ttf-parser 0.25.1 | `<- lopdf 0.42.0 <- pdf-extract 0.12.0 <- wcore-tools` | lopdf 0.44 makes ttf-parser optional, but pdf-extract 0.12.0 (latest) pins `lopdf ^0.42` = `>=0.42.0,<0.43.0`. Reaching 0.44 needs a forked `[patch]`. |
| RUSTSEC-2024-0436 | paste 1.0.15 | two roots, both `<- wcore-memory` (candle SIMD; tokenizers) | build-time proc-macro, no runtime surface; optional default-OFF `bge-local`. |
| RUSTSEC-2025-0119 | number_prefix 0.4.0 | `<- indicatif <- hf-hub <- wcore-memory` | indicatif pulls it at every published version; optional default-OFF `bge-local`. |
| RUSTSEC-2025-0134 | rustls-pemfile 2.2.0 | two edges, one root: `<- bollard <- wcore-sandbox` | needs bollard 0.17 -> 0.21 (four breaking majors) on a non-shipping optional backend. |

**Why non-blocking.** All five are informational-unmaintained with no known vulnerability and
no patch in existence. Three of the five (`paste`, `number_prefix`, `rustls-pemfile`) are not
in the shipped binary at all — they are absent from the default-feature graph.

**The one with real reach, stated plainly:** `ttf-parser` parses embedded font tables out of
**user-supplied PDFs** via the Read tool. That is untrusted input. It is accepted only because
there is nothing to apply. **If a concrete `ttf-parser` CVE is ever published, the exception
must be DELETED and the PDF path re-examined — not re-justified.**

**Action on each dependency bump.** Re-run `cargo deny --manifest-path Cargo.toml check` and
delete any entry that has cleared. Re-derive every trace from `cargo tree -i <crate>@<ver>`;
never carry a trace forward on trust. Two of the pre-existing dispositions in
`.github/osv-scanner.toml` were found WRONG by exactly this re-derivation (see below).

### `BL-F29-OSV-TRACES-WERE-STALE` — two dispositions carried untrue parent traces · MEDIUM · NON-BLOCKING

Found and **fixed in the same lane** (per the standing rule that a written-up instrument defect
is a defect you have agreed to keep). Recorded here because it is a recurrence, not a one-off:

- **paste (RUSTSEC-2024-0436)** claimed pullers "the candle SIMD stack ... **AND ratatui**".
  `cargo tree -p ratatui --all-features -e normal | grep -c paste` -> **0**. It named a parent
  that does not exist, and omitted the `tokenizers` root that does.
- **rustls-pemfile (RUSTSEC-2025-0134)** claimed "transitive **ONLY** via bollard". There are
  **two** direct parents (`bollard` and `rustls-native-certs 0.7.3`). The conclusion survives
  only because rustls-native-certs' own sole parent is also bollard — which the entry had not
  checked.
- **proc-macro-error (RUSTSEC-2024-0370)** carried a cost estimate ("a breaking major ... not
  worth the REST-surface regression risk") that was a claim about the dependency graph, never
  read out of it. The bump turned out to need **zero** source changes. That entry is now
  deleted because the advisory was eliminated at source.

**The pattern.** All three are the same defect the quick-xml entry had: a justification written
once and carried forward across files without being re-derived. **Any exception review must
re-run `cargo tree -i` and state the edge count**, and any "it would be a breaking change" cost
claim must be measured before it is believed.

### `BL-F29-DENY-GRAPH-SCOPE` — resolved, recorded so the reasoning is not lost · LOW · NON-BLOCKING

`deny.toml` had `[graph] all-features = false`, so the gate evaluated only the default-feature
graph. Measured: three unmaintained transitives (`paste`, `number_prefix`, `rustls-pemfile`)
were invisible to it, and — more importantly — **no gate in this repo checked the LICENSE of a
dependency behind an optional feature.** `cargo audit` and `osv-scanner` read the whole
lockfile but neither checks licenses, so a GPL/AGPL crate arriving via `hf-hub` or `bollard`
would have passed everything. Flipped to `all-features = true` after measuring the cost
(`cargo deny --all-features check licenses bans sources` -> exit 0; advisories -> exactly the
3 named errors). Cross-audit was 2-1 in favour; the dissent is recorded in `deny.toml`.
### `BL-F30-REFCOUNT-GATE` — the retained-ref gate counts remote-tracking refs, so its floor is meaningless (MEDIUM)

30-04's `RETAINED-EVIDENCE-REFS` gate asserts `git for-each-ref | grep -cv '^refs/heads/'` is at
least **37**, the count measured at planning. In the lane worktree that expression reads **275**,
because it includes **238 remote-tracking refs** the planning measurement did not. So the gate
passes with 238 refs of headroom and **could not detect the deletion of any of them**. The *tight*
count — `refs/tags` (36) plus `refs/f20a/*` (1) — is **37 exactly**, reproducing the baseline. The
tight count is the meaningful one and it is what the gate should assert, as an equality-or-floor
over `refs/tags` and `refs/f20a` rather than over everything that is not a branch. Both figures
are recorded in `evidence/30-04/captures/auth-06-ref-count-two-ways.txt`. Not blocking: the tight
count was taken and is exact.

### `BL-F30-FORCED-MET-SED` — 30-04's MET-forcing gate seds a field that does not exist (MEDIUM)

The `MET-IS-STILL-EXPENSIVE` gate forces a grade upward with
`sed 's#"verdict": *"NOT_MET"#"verdict": "MET"#'`. The field `CriterionV1` actually carries is
**`grade`**, not `verdict`. Run literally the sed is a **no-op** (measured:
`PLAN_SED_IS_A_NO_OP=YES`), producing an unchanged document that verifies successfully, whereupon
the gate reports `UNEXPECTED_MET_ACCEPTED` and exits 9 — failing, but for entirely the wrong
reason, and telling a reader the asymmetry is broken when it is not. 30-04 ran the corrected form
(`"grade"`), forced 2 grades, and captured the real refusal. Any future plan reusing this gate
shape must sed `grade`.

### `BL-F30-VERDICT-VERIFY-ARG` — 30-04's verdict-verify gate passes `--root`, the CLI takes `--repo-root` (LOW)

`wayland-scorecard verify` declares `--document` and `--repo-root`. The plan's gate passes
`--root .`, which clap rejects. Run as written the gate fails at argument parsing, which is safe
but uninformative. 30-04 ran the corrected form.

### `BL-F30-VACUOUS-MAIN-GATE` — "no local main contains HEAD" passes vacuously (MEDIUM)

30-04's `NO-MAIN-MERGE` gate asserts no local branch named `main` or `master` contains the lane's
HEAD. **There is no local `main` branch in this repository at all** (`LOCAL_MAIN_EXISTS=0`), so
the gate passes at base, passes after any amount of work, and would still pass if the lane had
merged to main on the remote. The plan conjoins it with a completion anchor, which proves the task
ran but does not make the check mean anything. The determination that actually carries is the
remote one: `ls-remote gh refs/heads/main` plus an ancestry test with a falsification control
(`evidence/30-04/captures/auth-07-remote-main-observability.txt`). Future plans should assert
against the **remote** ref, not a local branch that does not exist.

### `BL-F30-AUDIT-CEILING-PREMISE` — the audit ceiling was stated on a false premise (MEDIUM)

30-04's `read_first` states that *"this repository has NO remote-tracking refs at all, which is
what bounds the audit."* There are **238**, and `refs/remotes/gh/main` is among them, its cached
SHA matching a live `ls-remote` exactly. The conclusion (a residual ceiling exists) survives, but
the reason is different and the ceiling is **narrower** than the plan assumed: the main-merge half
is measurable and was measured. What genuinely cannot be observed is everything writing no git
object — pull requests, issue closures, release publications and deployments. A plan asserting a
limitation should measure the limitation before asserting it.

### `BL-F30-ROADMAP-STALE-STATUS` — the ROADMAP progress table contradicts the tree for Phases 28 and 29 (MEDIUM)

`.planning/ROADMAP.md`'s Status column states for Phase 28 *"IN PROGRESS — no phase verdict exists
yet"* and *"28-04 not started"*, and for Phase 29 *"29-03 and 29-04 not started"*. All four
artifacts exist on disk: `28-04-PHASE-VERDICT.md` (22.1K), `28-04-SUMMARY.md`,
`29-PHASE-VERDICT.md` (20.4K), `29-04-SUMMARY.md`. This is 30-01's STALE-06/07/08 still
unrepaired. It matters beyond tidiness because Phase 30's own verdict quotes its Success Criteria
from this file — the **criteria text is current**; only the progress table is stale. 30-04 graded
against the artifacts on disk, not against the table, and did not edit ROADMAP.md from inside the
phase being graded.

### `BL-F27-MCP-DISCOVERY-NAMING` — a tool's host-visible name depends on what else is in the session (MEDIUM)

`27-GAPS-SUMMARY.md:99` and `evidence/27-gaps/c3-generation/README.md:80` both record this
finding as dispositioned "to BACKLOG per the standing severity policy". **It was never filed** —
before this entry, `.planning/BACKLOG.md` contained zero Phase 27 rows, so a MEDIUM that the
policy makes non-blocking *by filing it* was simply dropped. Filed now (INV-26-27 BLOCKER-27-M1).

Measured, same fixture, same two tools: shape B (config server alone) and shape C (late server
alone) each announce `media_generate_image` / `media_generate_locked`. Shape D (both) announces
the **late** server's tools under the bare names and renames the **config-declared** server's to
`mcp__f27media__media_generate_image`. So a host that learned `media_generate_image` from a
config server in one session sees the same server's tool under a different name in the next,
purely because a late server carried a colliding name — and `RemoveMcpServer`'s own doc comment
says configured servers "remain authoritative", which is the opposite of how the collision
resolves. Prefixing on collision is sound; which side gets prefixed looks inverted.

MEDIUM, not higher: names stay unique and functional within a session, nothing is silently
dropped, and no security or correctness property breaks. Criterion 27-C3 cannot honestly be
called fully met while it stands, because the criterion's word is "consistent". Cost 0.5.

### `BL-F27-FLUX-IMAGE-DEFAULT-ARM-401` — `wayland-core image` default arm returns 401 on a cleared paid key (HIGH, argument for MEDIUM)

Measured live on `hetzner-dsm` with a cleared paid Flux key, same key and same run:

```
wayland-core image --prompt "..." --out a-default.png
  -> rc=1  "image generation failed: API error 401: {"error":{"message":"unauthorized"}}"  no artifact
wayland-core image --model flux-image --prompt "..." --out a-flux.png
  -> rc=0  wrote a-flux.png (46216 bytes)  JPEG 1024x1024
```

`flux_image.rs:31` sets `DEFAULT_IMAGE_MODEL = "flux-image-together-flux"`; the key's entitlement
list contains `flux-image` and not that arm. The user is therefore told their **credential** is
unauthorized when the credential is fine and only the default arm is not entitled, and the
subcommand's own help promises a distinct `premium_locked` message for exactly this situation —
which never fires. There is a precedent for the honest form: `image_gen.rs:403` signposts
`OPENAI_IMAGE_MODEL` when a model is unavailable. The CLI image path has no equivalent.

Graded HIGH by the precedent of 27-C2(a) (an advertised remedy naming the wrong thing) with an
explicit argument for MEDIUM: a working `--model` escape exists, so the user is not left with
zero options. **Residual uncertainty, stated rather than hidden:** whether a different Flux plan
entitles `flux-image-together-flux` is not knowable from this side — only Sean can confirm. If
it normally is entitled, this drops to MEDIUM (a bad error message on an unusual plan).
Evidence: `.planning/phases/27-*/evidence/27-credentialled/shape-a-live-hetzner.log`.

### `BL-F27-FLUX-PROVIDER-PREFIX-UNSUPPORTED` — `-m flux-router:flux-fast` boots into anthropic and blames the wrong key (MEDIUM)

With `FLUX_API_KEY` in the environment **and** a populated `[providers.flux-router]` block,
`-m flux-router:flux-fast` fails at init:

```
init_failed: No API key found. Provide via --api-key, config file, or environment variable
(API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY). Provider 'anthropic' requires an API key. To use
a LOCAL model with Ollama, select a model id prefixed with `ollama:` ...
```

The `provider:model` prefix works for `ollama:` and the message advertises it, so the form looks
general. It is not. The working invocation is `-p flux-router -m flux-fast` plus a `[default]`
block. The message names neither Flux nor the credential that was present. Cost 0.25 — either
accept the prefix for registered providers, or name the configured-but-unselected provider in
the error. Evidence: `evidence/27-credentialled/27-NOTES.md`.

## BL-23B-H1 — session journal read-back mismatch (MEDIUM, non-reproducing)

**Source:** `23B-H1`, originally graded HIGH. **Disposition 2026-07-29: MEDIUM, backlog.**

Does not reproduce at HEAD, at the pre-fix binary, or at 23B-01's own base commit: **92 runs, 153
tool events, 0 mismatches**, under CPU load to 114, 6-way concurrency, a turn cut mid-flight, and
fsync saturation at 11,139 IOPS.

**Why the earlier evidence cannot be relied on in EITHER direction:** the inherited reproduction
harness pointed at `http://127.0.0.1:1` with a placeholder key, so **no run ever dispatched a tool
event** — and it had no bucket for that, folding non-reaching runs into `resume_ok`. All 46 prior
non-reproductions were produced that way.

**This is a non-reproduction, not a disproof.** Root cause remains unidentified. Excluded by
measurement: the previous fix is not what changed the outcome, and the original base code state is
not sufficient. Excluded by reading: the textbook cause of this signature, plus six other serde
shapes. **Residual:** the original sighting's provider configuration was never recorded.

**Re-escalate on any fresh sighting** — the reach-proven harness is `scripts/f23-h1-repro-live.sh`,
which emits `F23_H1_REACH=` per run so a non-reaching run can never again be counted as a pass.

**Separate, real, and not built:** an unreadable journal has no repair path. Only
`recover_legacy_effect_receipt` exists, keyed literally to a null receipt, and **all twelve `session`
verbs read the journal — so one mismatch takes every operator move down at once.**

## BL-F24-C3-H7 — inbound vision is unreachable by code absence, not capability absence (MEDIUM)

**Source:** `24-media-live`, 2026-07-29. Report:
`.planning/phases/24-gateway-automation-channels-typed-api/24-MEDIA-LIVE.md`.

The predecessor lane graded the live vision leg "unreachable with the available credential". That
verdict still holds, but **the reason it gave was wrong**, and the corrected reason is actionable
where the original was not. Re-measured rather than inherited:

- `build_vision_backend()` **takes no `&Config`** and reads only ANTHROPIC / OPENAI / GEMINI.
  **Zero Flux sites workspace-wide** (control: `transcription_backend_from_config` = 7 refs, so the
  instrument discriminates).
- `OpenAiVisionBackend` posts to a **hardcoded `openai.com` URL**. Substituting a key would
  therefore **misdirect the credential to a third party** rather than fail closed — the reason this
  must not be worked around by key substitution.
- Flux **does** serve vision on the same OpenAI wire: proven live, HTTP 200, ground truth recovered.

So the blocker is ours, not the vendor's: the config seam that transcription already has does not
exist for vision. Same shape as the transcription resolver at `tool_backends/mod.rs:344` before it
was extended.

**Not fixed** — the finding landed at end of lane and a blind change there would have been
unproven. Cost is small and bounded: give `build_vision_backend()` the config seam its sibling
already has, then the existing live probe closes the leg.

**Routed here because the lane's own report was its only home** — that is the findings-leak class
(20 dropped findings recovered on 2026-07-28, two of them HIGH).

## BL-F24-MSTEAMS-H1 — `wcore-channels::media` was advertised-but-dead (MEDIUM)

**Source:** `24-msteams-attach`, 2026-07-29. Report:
`.planning/phases/24-gateway-automation-channels-typed-api/24-MSTEAMS-ATTACH.md`.

The module header says **"never drop silently"**. Measured at `15cda12d`:

| symbol | production call sites |
|---|---|
| `media_bounds()` | **1 — and it is a test** |
| `normalize_all()` | **2 — its own definition and its own unit test** |
| `max_message_len()` *(control)* | **7, with real production callers** |

The control is what makes those zeros a measurement rather than a dead grep. **Four adapters that
parse attachments bypass the module entirely.** This is the same defect `24-media-actions` filed as
`F24-C3-H6` from the opposite direction — two lanes, two routes, one surface.

**Partly closed already:** msteams is now the module's **first and only production consumer**
(`wcore-channel-msteams/src/inbound.rs` → `normalize_all(&candidates, Channel::media_bounds())`).
`lane/24-media-bounds` is making the declaration load-bearing for the remaining adapters and has
been told msteams is now a live known-positive it can use rather than build.

**Residual after that lane lands:** the four bypassing adapters, and the wire gap below.

**Fenced seam request (NOT actioned — needs the contract train):** `wcore_channels::Attachment`
has **no disposition field**, so when an attachment is dropped or degraded the *reason* cannot
reach the agent. The agent sees a shorter list and cannot tell absence from rejection. Adding it is
a wire change and must ride the contract regeneration that is owed before any tag — it must not be
done ad hoc.

**Also corrected by that lane:** a fourth stale advertised-but-dead site — the operator-facing
msteams config schema still read *"send-only MVP; inbound webhook receive is deferred to v0.8.3"*,
long false. That family now stands at nine recorded instances.
