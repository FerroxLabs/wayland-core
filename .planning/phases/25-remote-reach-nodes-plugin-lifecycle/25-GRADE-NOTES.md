# Phase 25 — GRADE NOTES (running log, lane `grade-25`)

Started 2026-07-29. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-25`,
branch `lane/grade-25`, base `861d1b1a716240165209336b1fa38d36f9445716` (verified with
`/usr/bin/git rev-parse`).

**Mandate:** Phase 25 has NO verdict file. Produce `25-PHASE-VERDICT.md` grading all four
ROADMAP Success Criteria. Verify existing evidence rather than inherit it. Re-derive all
arithmetic. Grading only — no `crates/`, no workflows, no build.

---

## Success Criteria (verbatim from `.planning/ROADMAP.md` lines 124-136)

**Goal:** Operators can run governed work across reference backends/nodes and manage plugins
through a complete, recoverable lifecycle.

1. The same task runs locally, in a container, over SSH, and on one hibernating cloud backend
   with equivalent policy, receipts, cancellation, and cleanup.
2. Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions
   without losing authority attribution.
3. Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated,
   rolled back, removed, published, and recovered.
4. Compromised keys/plugins/backends and denied secret/egress paths fail closed with no
   orphaned execution.

Requirements listed: F25-01..F25-05. **Note: ROADMAP lists FIVE requirements (F25-05
included) but only FOUR Success Criteria and four plans (25-01..25-04). F25-05 has no
criterion and no plan — flagged for the verdict as a scope question.**

---

## Prior claims to verify (NOT inherit)

`25-PHASE-STATUS.md` header table claims **all four MET**:
- C1 MET (lane/25-cloud 2026-07-28)
- C2 "MET on every named property, one limitation" (lane/25-hosts)
- C3 MET on Linux, PARTIAL on Windows
- C4 MET (lane/25-hosts)

But the SAME file's "graded verbatim" section (written 2026-07-27, partially superseded)
says **C2 NOT MET** and **C4 NOT MET**. The file itself acknowledges the header "used to
claim two of four MET while the verbatim gradings showed only Criterion 3."

The competitive ledger reportedly records `REACH-*` SOURCE -> REACHED and calls it "the only
family carrying a MET Success Criterion" — i.e. **exactly one MET**. That is in direct
conflict with the status file's four-MET table. **Resolving this conflict is the core of this
lane's job.** Three mutually inconsistent records exist:
  (a) verbatim 2026-07-27 gradings: 1 MET (C3, and only "on Linux")
  (b) status header table 2026-07-28: 4 MET
  (c) competitive ledger: exactly 1 MET

## Instrument warnings in force

- nextest "flakiness" here = fd exhaustion; 40 runs, 0 real failures. Any red `exec failed`
  is NOT a regression.
- `.config/nextest.toml` `no-tests = "fail"` is SILENTLY IGNORED by installed nextest ->
  a green suite may have run zero tests. Downgrade confidence wherever a criterion rests on it.
- A known-negative assertion (orphan count == 0, "no fallback", "no leak") is SELF-PASSING
  on a dead instrument. Every zero in this phase's evidence needs a known-positive in the
  SAME invocation. Phase 25's own history contains exactly this defect twice (finding #9,
  Windows MEASURED ZERO while orphan ran; and the cloud nonce-filter structural false zero).
- `rtk` rewrites `git log` / `grep` / `cargo` / `wc -c`. All load-bearing reads via
  `/usr/bin/`.

## Evidence inventory (present on disk, byte counts pending re-derivation)

Phase dir has: 25-01..25-04 PLAN/SUMMARY, plus 25-01-CLOUD-BACKEND-DECISION,
25-01-EQUIVALENCE-EVIDENCE, 25-02-CLI-GATE-DECISION, 25-02-LIFECYCLE-TRANSCRIPT,
25-03-NODE-EVIDENCE, 25-04-FAIL-CLOSED-EVIDENCE, 25-CLOUD-SUMMARY, 25-HOSTS-SUMMARY,
25-MACOS, 25-PHASE-STATUS. `evidence/` holds ~100 capture files plus subdirs
`25-01/`, `25-cloud/`, `25-macos/`.

## Working log

- [t0] Worktree + branch verified. ROADMAP criteria extracted verbatim. Conflict between
  three records identified. NOTES committed.
- [next] Read 25-01/25-CLOUD-SUMMARY + equivalence + cloud ledger -> grade C1, with
  specific attention to whether the SSH leg was ever run at the SAME commit as the other
  three (status file itself qualifies this as "a composition across two commits").

---

## MEASUREMENTS (re-derived, unproxied tools)

### C1 — four surfaces
Evidence read: `25-01-SUMMARY.md`, `25-CLOUD-SUMMARY.md`, `evidence/25-01-equivalence-ledger.txt`,
`evidence/25-cloud-ledger.txt`.

- 25-01 ledger line 10 is the executor's OWN verdict: `F25-SC1-VERDICT: NOT-MET`. Three of four
  surfaces ran (local/container/ssh), cloud NOT-RUN, credential absent.
- 25-cloud ledger: cloud RUN PASS, receipt integrity PASS, hibernation observed as genuine
  SUSPEND with a stop/start control on ONE machine, provenance gate passes. `F25-SC1-VERDICT: MET`.
- **SSH not re-run at the cloud commit** — the cloud ledger states this itself
  (`F25-SC1-SSH-AT-THIS-COMMIT: NOT RUN`). Four-surface claim is a composition across two commits
  (`5e620ef0` and 25-01's commit).
- **CANCELLATION ON CLOUD WAS NEVER EXERCISED — on any commit.** Criterion 1 names
  "policy, receipts, **cancellation**, and cleanup" as the equivalence set.
  - 25-01: `F25-SC1-CLOUD-CANCEL: NOT-RUN` (line 8), reason=no credential.
  - 25-cloud: no CLOUD-CANCEL marker exists at all.
  - Measured with a live-instrument control:
    `grep -c "F25-SC1-CLOUD-RUN" evidence/25-cloud-ledger.txt` -> **1** (instrument alive)
    `grep -c "CLOUD-CANCEL" evidence/25-cloud-ledger.txt` -> **0**
    `grep -rn -i "cancel" evidence/25-cloud-{live-run,provenance,orphan-control}.txt` -> **0**,
    while `grep -rc "machine" evidence/25-cloud-live-run.txt` -> **3** (instrument alive on the
    same files).
  - `25-CLOUD-SUMMARY.md` §"What this lane did NOT do" concedes it: "Did not exercise cloud
    cancellation live."
- **CLEANUP is defective on the ssh surface.** `25-HOSTS-SUMMARY.md` FINDING 5 (MEDIUM, BACKLOG,
  UNFIXED): the ssh remote runner leaves its task root behind on failure — `set -e` aborts at
  `wait` so `rm -rf "$root"` never runs, leaving `input.bin` (the task's INPUT BYTES) on the far
  end. Six such roots were found on the node and purged by hand. The lane itself says this
  "touches Criterion 1's 'cleanup'".
- SSH leg proof quality: `F25-SC1-SSH-TARGET: CONTAINERIZED-SSHD` — same physical host, and
  `backend.instance_id` identical across all three receipts. Honestly disclosed by 25-01.

### C2 / C4 source-level verification of the two disclosed limitations
Both verified in the actual tree, not inherited (known-positive control:
`grep -c "" crates/wcore-cli/src/node.rs` -> **623 lines**, instrument alive):
- FINDING 7 confirmed: `crates/wcore-cli/src/node.rs:395` -> `let _ = ad;` — the
  `NodeAdvertisement::observe` result at line 381 is computed and DISCARDED. `node probe` never
  refreshes the advertisement.
- FINDING 4 confirmed: `crates/wcore-cli/src/node.rs:507` `backend_key_from`, and `:512`
  "this host does not hold the signing key for backend '{}'" — the controller genuinely cannot
  verify a node-minted receipt's IDENTITY.

### C3 — ARITHMETIC RE-DERIVED, and the criterion does not say twelve
Criterion 3 names: scaffolded, tested, signed, installed, approved, inspected, updated,
rolled back, removed, published, recovered = **ELEVEN** verbs. The phase calls it a
"twelve-verb lifecycle"; the twelfth (`verify`) is a superset addition, not a criterion item.
- Linux ledger (`evidence/25-02-lifecycle-ledger.txt`): 12 verb lines, all PASS + 4 negative
  cases PASS. All ELEVEN criterion-named verbs are covered.
- Windows ledger: `F25-SC3-WIN-VERB-NEW: NOT-RUN` (cargo-generate absent) and
  `F25-SC3-WIN-NEG-APPROVED-LOADS: PARTIAL`.
- **DISCREPANCY FOUND:** the LINUX ledger's last line claims
  `F25-SC3-WINDOWS: PASS ... reason=full-twelve-verb-drive-ran` — but the Windows ledger it
  points at records 11 of 12 with `new` NOT-RUN. A ledger line overstating the evidence it
  cites. Recording it.
- Criterion 3 names NO platform (unlike Phase 24 SC5), so Linux-only satisfies it as written.

### Still to establish
- C3: does `plugin test` actually run tests, or exit 0 having run zero? (vacuity class)
- C4: were all five hostile compromises INDUCED FOR REAL on both hosts?
- Whether F25-05 (listed in ROADMAP Requirements, no criterion, no plan) is a scope gap.

### C4 — THE CENTRAL FINDING
- Linux: denied-secret (log:64) and denied-egress (log:156) are the SAME command
  (`backend run --backend cloud`) with the SAME CredentialAbsent verdict. Four distinct
  mechanisms, not five.
- Windows: denied-egress capture is `backend probe cloud` **EXIT: 0**, while the ledger
  records `REFUSED ... exit=1`. Enumerated all 6 commands in the Windows log — there is NO
  `backend run` egress leg. The Linux leg was corrected and re-run; Windows never was.
  This is lane-brief 6b-ii exactly: documented defect that recurred because the instrument
  was not repaired everywhere.
- `25-04-FAIL-CLOSED-EVIDENCE.md` §6 concedes: "A real egress policy denial ... is NOT proven."
- Both hosts DO carry a vacuity control ("verify the intact receipt (must PASS or the cases
  are vacuous)") — good discipline, recorded in the verdict's favour.
- Orphan half is genuinely MET on all surfaces, both directions, with UNPLANTED positives.

### Credential answer (asked specifically)
`/root/.wayland-f25-cloud.env` EXISTS on hetzner-dsm, 0600, 716 bytes, 3 lines, Jul 28.
Names both WAYLAND_F25_CLOUD_TOKEN and WAYLAND_F25_CLOUD_ORG (count 1 each). Known-negative
control `ZZZ_NOT_A_REAL_VAR` -> 0, so the instrument can return zero. **No value printed.**
=> the cloud-cancellation gap is BUILD WORK, not a Sean item.
`WAYLAND_EXEC_SSH_TARGET` verified UNSET on hetzner-dsm today.

### FINAL GRADES
C1 PARTIAL · C2 MET-WITH-STATED-EXCEPTIONS · C3 MET · C4 PARTIAL
=> 1 MET. Corroborates the competitive ledger; refutes the 4-MET status table.

### Fence exposure, final (vs 861d1b1a)
Committed by this lane: 25-GRADE-NOTES.md, 25-PHASE-VERDICT.md. Both under
.planning/phases/25-*/. crates/ touched: 0 files. .github/ touched: 0 files.
Shared-fence files (wcore-cli lib.rs/main.rs): 0 diff bytes.

**One incidental dirty file, NOT committed and NOT mine:** `AGENTS.md` was modified in the
working tree by the IJFW hook that fires on a `gemini` invocation (cross-audit panel §4). The
change is 3 frontmatter lines only — `detected_at` timestamp and a file-extension-ratio
recount (1522 -> 1775). Left uncommitted and unreverted: the lane brief forbids
`git checkout`/`git reset` because lanes share the object store. Reported rather than
silently cleaned. Any lane running the panel will reproduce it.
