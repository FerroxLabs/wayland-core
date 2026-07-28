---
phase: 30-continuous-scorecard-frontier-review
plan: "04"
subsystem: eval-harness
status: complete
termination_state: 2 (complete with criteria NOT MET)
tags: [reserved-authority, closed-enum, fail-closed-placeholder, phase-verdict, F30-05]
requires: ["30-01", "30-02", "30-03"]
provides:
  - "reserved authority as nine closed actions, one principal that is not the agent, and nine per-action signature domains"
  - "a bundled all-zeros approval root that fails closed, making every reserved action structurally unreachable here"
  - "both authority runs on hardware: the mechanism accepts a valid approval, and no approval exists in this lane"
  - "the four Phase 30 Success Criteria graded verbatim through the real verifier"
  - "a positioning packet containing no positioning sentence"
affects: []
tech-stack:
  added: []
  patterns:
    ["closed-enum-anchored-to-a-real-incident", "structural-unreachability-by-placeholder", "verbatim-grading", "positive-control-is-mandatory"]
key-files:
  created:
    - crates/wcore-eval-scenarios/src/reserved_authority.rs
    - crates/wcore-eval-scenarios/tests/reserved_authority_contract.rs
    - .planning/phases/30-continuous-scorecard-frontier-review/30-04-AUTHORITY-PROOF.md
    - .planning/phases/30-continuous-scorecard-frontier-review/30-04-POSITIONING-PACKET.md
    - .planning/phases/30-continuous-scorecard-frontier-review/30-PHASE-VERDICT.md
    - .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-04/
    - .planning/SEAM-REQUESTS/30.md
  modified:
    - crates/wcore-eval-scenarios/src/lib.rs
    - crates/wcore-eval-scenarios/bin/wayland-scorecard.rs
    - .planning/phases/30-continuous-scorecard-frontier-review/30-02-SUMMARY.md
    - .planning/BACKLOG.md
decisions:
  - "Criterion 4 graded PARTIAL on a 2-1 cross-audit; kimi's UNPROVEN dissent recorded with the reason it did not carry."
  - "Criterion 1 and 2 both NOT_MET rather than PARTIAL; the dissent for each is recorded with the numbers to overturn it."
  - "F-30-03-001 fixed at source in 30-02-SUMMARY.md — prose only, evidence/30-02/ gate-checked untouched."
  - "The plan's audit ceiling premise was measured FALSE: 238 remote-tracking refs exist and gh/main is among them, so the main-merge half was measured rather than declared unobservable."
metrics:
  reserved_actions: 9
  signature_domains: 9
  principals: 1
  contract_tests: 14
  targeted_suite: "505 passed, 0 failed, 5 skipped"
  audit_determinations: 10
  criteria_met: 0
  criteria_not_met: 2
  real_key_limits: 8
---

# Phase 30 Plan 04: Reserved authority and the phase verdict — Summary

Made inventing a way around Sean's approval a **deserialization failure** rather than a policy
violation, proved the mechanism both ways on hardware, and then graded Phase 30's four Success
Criteria verbatim. **Two are NOT MET, one is PARTIAL, one is MET WITH STATED EXCEPTIONS, and the
phase goal is NOT ACHIEVED.**

**Termination state: 2 — complete with criteria NOT MET.** Every clause of state 1 is also
satisfied: the module landed green, both authority runs are captured, the audit ran with its
ceiling stated, and all four criteria are graded with evidence. State 2 is the honest label
because the substantive outcome is the *grades*, and the plan itself named this as the likely
one.

**This plan made no positioning decision and contains no recommendation. That omission is the
requirement, not an oversight.**

---

## 0. The headline

**The frontier position cannot yet be stated.** Not because the evidence is unflattering —
because there is no comparative evidence at all. Zero of the five comparative dimensions
produced a usable result: two never ran, and the three that ran are confounded by a script that
speaks one competitor's tool dialect. 30-03 published **zero peer comparisons in either
direction** and that refusal was correct.

What it would take to state it is listed in `30-PHASE-VERDICT.md` §2 in ascending cost. **The
cheapest item — per-tool dialect compilation and a re-pre-registered protocol v2 — needs no
credential, no account and no authorisation from anybody, and without it Criterion 2 cannot be
re-graded at all.**

## 1. RED before GREEN, both read back from the executed count

| | |
|---|---|
| RED commit (contract suite alone) | `af8e12b9` |
| RED result | `RED_RC=101`, `error[E0432]: unresolved import wcore_eval_scenarios::reserved_authority` |
| GREEN commit | `5602ee11` |
| GREEN result | **14 tests run: 14 passed, 0 failed, 0 ignored** |
| oracle correction | `db9c69d0` |
| final targeted suite | **505 run: 505 passed, 0 failed, 5 skipped** |

Suite delta accounted for exactly: 30-03's baseline **485** + 14 contract + 6 inline = **505**.
No residual failure.

### My own inline test failed, and the code was right

`civil_from_days_matches_known_dates` asserted day 11016 was 2000-03-01. The implementation
answered **2000-02-29**, and the implementation was correct — 2000 divides by 400 and *is* a leap
year, the exact case a naive four-year rule gets wrong. **The oracle was wrong, not the code.**
The oracle was corrected — not the code, and not by deleting the assertion — and the boundary is
now pinned from both sides, plus the 1900 non-leap case and two negative days that exercise the
Euclidean-remainder path a plain `%` would get wrong.

## 2. What landed

**Nine reserved actions, closed.** `source_push`, `main_merge`, `pull_request`, `tag`, `release`,
`deployment`, `issue_closure`, `retained_evidence_ref_deletion`, `frontier_positioning`. No
catch-all, no default, no untagged fallback, backed by `[ReservedActionV1; 9]` whose length the
**compiler** checks. `an_invented_reserved_action_named_termination_state_4_fails_to_deserialize`
feeds it the literal string an agent on this program actually invented, and that literal is
gate-checked **absent** from the production module.

**One principal, and it is not the agent.** `PrincipalV1::Sean` and nothing else. Neither an
agent principal nor a self-approval principal appears anywhere in the module — gate-checked at
zero — and both are proved to fail deserialization by name.

**Nine distinct signature domains**, counted distinctly (`grep -oE … | sort -u` → **9**), so nine
repetitions of one domain would not satisfy the gate. Approving a documentation push is not
approving a release.

**A bundled root that fails closed.** `APPROVAL_ROOT_PUBKEY_HEX` is 64 zeros. Verification
refuses every approval with an explicit **F-030** error naming its substitution point. This
copies the shape `IndexVerifier::bundled()` already proves (F-021) rather than improvising —
and the precedent is regression-guarded at its measured baseline, **`F-021` = 6,
`INDEX_PUBKEY_HEX` = 6**, exactly as the plan declares.

## 3. Both authority runs, in the required order

**The mechanism works.** Positive control, run **first**:

```
AUTHORITY_INIT_ROOT=OK root_kind=throwaway_generated_at_run_time
key_id=throwaway-not-seans-key-31007056bb20fee7 public_key_hex=31007056bb20fee7…
600 /tmp/tmp.cxaic9AVXN/throwaway-not-seans-key-31007056bb20fee7.seed
AUTHORITY_VERIFY=ACCEPTED action=frontier_positioning principal=sean
  subject_sha256=d89c9063… root_kind=throwaway_generated_at_run_time
NOTE: root_kind is not operator_supplied. This acceptance proves the MECHANISM works;
      it is not an approval and authorises nothing.
POSITIVE_CONTROL_RC=0
```

**No approval exists in this lane.** The same approval against the committed placeholder root,
remote exit status **0** (which under this gate's shape means the refusal happened):

> `wayland-scorecard: reserved action `frontier_positioning` is unreachable: the approval trust
> root declares key `sean-reserved-approval-root` as the all-zeros placeholder, which authorises
> nothing. Substitution point: replace APPROVAL_ROOT_PUBKEY_HEX in
> crates/wcore-eval-scenarios/src/reserved_authority.rs with Sean's real Ed25519 approval public
> key. Until that substitution is made, every reserved action including frontier positioning is
> structurally unreachable from this repository. (F-030)`

**Both sentences are stated because reporting the first without the second is precisely how a
lane would drift into acting as though it had approval.** The refusal gate lets the remote side
decide — setup failures exit non-zero, an unexpected acceptance exits 9, only the refusal exits 0
— because asserting a non-zero status locally also passes when ssh is down.

**No seed disclosure**, measured live with a control: `SEED_LEN=44`, `SEED_LEAKED=NO`,
`CONTROL_GREP=FOUND` (the identical grep *does* find a string that is in stdout), file mode
**600**. The binary declares no clap argument carrying `seed`, `private` or `secret` — zero,
red at base.

## 4. The audit — and the plan's stated ceiling was wrong

**10 determinations, 10 well-formed of 10**, each naming a capture that exists.

The plan's `read_first` states *"this repository has NO remote-tracking refs at all, which is what
bounds the audit."* **False against the tree: there are 238, and `refs/remotes/gh/main` is among
them**, its cached SHA matching a live `ls-remote` exactly. So the main-merge half was
**measured**, not declared unobservable:

```
HEAD_IS_IN_REMOTE_MAIN = NO
CONTROL (main tip vs main) = YES   -- the check CAN answer YES, so the NO is a real measurement
```

That is a better outcome than the plan predicted and is reported as a **correction to the plan**.

**What remains genuinely unobservable** is everything writing no git object: pull requests
(`refs/pull` count 0), issue closures, release publications and deployments — for **any** actor,
over the whole phase. Recorded as two `NOT-OBSERVABLE-HERE` determinations. **No credential was
obtained to close it.**

**AUTH-03 is a weak confirmation and is flagged as one.** There is no local `main` branch at all,
so "no local main contains HEAD" passes vacuously — it would pass at base and after a remote
merge. AUTH-07 is what carries it. Filed as `BL-F30-VACUOUS-MAIN-GATE`.

## 5. The four Success Criteria, graded

Quoted verbatim in `30-PHASE-VERDICT.md`, machine-verified: `SCORECARD_VERIFY=OK criteria=4`,
rc=0, at the final SHA.

| | Grade | What settled it |
|---|---|---|
| **1** per-surface truth refreshed at each phase | **NOT MET** | `operator_completeness` and `peer_delta` UNPROVEN on **148/148**; 15 rows owned by no family; `forgeflows` is a live hidden alias with no row; one refresh is not a cadence |
| **2** five-dimension trials across three tools | **NOT MET** | security 0/3, cognitive tax 0/3; the nine legs that ran are confounded, so **zero** dimensions yielded a usable comparative |
| **3** claims match evidence, no unsupported superiority | **MET WITH STATED EXCEPTIONS** | re-render byte-identical, tamper DETECTED, publish REFUSED on a broken reference; 12 distinct rules fired; three exceptions, all named by 30-03 itself |
| **4** no reserved action without Sean's approval | **PARTIAL** | mechanism proved both ways, positioning demonstrably did not occur, this lane merged nothing — but issue closure, release and deployment are unobservable |

**Criterion 4 was cross-audited** (captures in `evidence/30-04/panel/`): codex **PARTIAL**, gemini
**PARTIAL**, kimi **UNPROVEN**. Kimi's dissent — PARTIAL implies a known partial *failure*, and
no clause is proven violated — is well made and did not carry, because UNPROVEN would erase the
affirmatively proven half and would say we do not know whether frontier positioning occurred. We
do. It did not. An internal adversarial pass argued *for* MET WITH STATED EXCEPTIONS and failed
on its own premise.

**Criteria 1 and 2 record their PARTIAL dissents with the numbers to overturn them.** Neither
grade was softened after seeing that it failed, and none was regraded.

## 6. The MET asymmetry, re-proved at the end of the phase

```
FORCED_COUNT=2
REFUSED_AS_REQUIRED
wayland-scorecard: criterion `CRIT-01` is graded MET but evidence reference
`OPERATOR-COMPLETENESS-148-OF-148` is marked UNPROVEN
```

Forcing both `NOT_MET` grades to `MET` is refused — **on the actual reason Criterion 1 failed**,
not on a synthetic one. The gate is structured so ssh failing, the checkout failing or the binary
being missing cannot satisfy it.

**The plan's own forcing gate would not have worked.** It seds `"verdict": "NOT_MET"`; the field
`CriterionV1` carries is **`grade`**. Run literally it is a no-op (measured:
`PLAN_SED_IS_A_NO_OP=YES`), which would have produced an unchanged document, a successful verify,
and a reported `UNEXPECTED_MET_ACCEPTED` — failing for entirely the wrong reason and telling a
reader the asymmetry was broken when it is not. Corrected form run and captured.

## 7. Gate results, with real numbers

| gate set | result |
|---|---|
| Task 1 local gates | **9/9 PASS**, and **7/7 RED at base** (`git archive fced9f61`) |
| Task 2 local gates | **6/6 PASS**; 10 well-formed determinations of 10; 2 `NOT-OBSERVABLE-HERE` |
| Task 3 local gates | **7/7 PASS**; 4 criterion rows of 4; 8 KEY rows; 18 PKT rows; readiness vocabulary **0** |
| nine distinct signature domains | **9** |
| `F-021` / `INDEX_PUBKEY_HEX` precedent | **6 / 6**, exactly the declared baseline |
| retained refs — tight count | **37**, exactly the baseline |
| verdict verify (hetzner, final SHA) | `SCORECARD_VERIFY=OK criteria=4 surfaces=0`, rc=0 |
| forced-MET refusal | `REFUSED_AS_REQUIRED`, rc=0 |
| targeted suite (hetzner, final SHA) | **505 run: 505 passed, 0 failed, 5 skipped** |
| new dependencies | **0** — `Cargo.toml` and `Cargo.lock` gate-checked untouched |

**Clippy is RED, and it is not mine.** 4 errors, all four in Phase 24's `journey.rs`
(683/695/707/717). I did **not** assume the abort spared my files: re-run without `-D warnings`
it completes rc=0 with **0 hits across all four of my files** and 4 in `journey.rs`. Not fixed,
not silenced, not attributed here.

## 8. Instrument defects measured in this lane

The standing pattern held again — the instrument that hunts a defect class carries it.

1. **My own falsification harness manufactured a self-passing gate.** `git show BASE:path > file`
   creates the file even when `git show` fails, so an absent-at-base file appeared
   present-and-empty and `test -f` passed. Six of seven gates still went red on their grep legs,
   which is exactly what hid it; `NO-SECRET-ON-ARGV` alone reported GREEN AT BASE and looked like
   a defective gate in the plan. Re-run with `git archive | tar -x`, **all seven are red**. The
   gate was sound; my falsifier carried the defect.
2. **`rtk` silently filtered `git for-each-ref`** — an interactive listing printed nothing while
   `grep -c` on the identical pipeline counted **2**. Same class 30-03 measured on `git log`. Not
   fabricated, *filtered*. Everything load-bearing here is file-captured and byte-counted.
3. **My own anchored regex lost every match.** Adding `%(objectname)` to the format put a SHA
   after the refname, so `/(main|master)$` matched **zero** where the unanchored form matched
   **two** — verbatim the trap the lane brief names, walked into while writing the check meant to
   catch it.
4. **The panel's first `GRADE=` match is the echoed prompt**, extracting as an empty vote. Taking
   the first match would have produced a silent 0-0-0 panel.
5. **`codex exec` blocks reading stdin** without `< /dev/null`. It produced 39 bytes
   (`Reading additional input from stdin...`) and timed out at 400s **twice** before
   byte-counting the capture found the cause.

## 9. Deviations, each with its reason

1. **Gates run in the lane worktree, not `/Users/seandonahoe/dev/waylandcore-ferrox`.** Every
   plan gate is written `cd /Users/seandonahoe/dev/waylandcore-ferrox && …`. This lane's changes
   live in the worktree; run as written the gates would test a tree without them. The intent is
   plainly "this lane's tree". `waylandcore-ferrox` was left **clean** (`FERROX_DIRTY_LINES=0`).
2. **Own hetzner worktree `/root/wayland-30-04`** rather than `cd /root/wayland`. Multiple lanes
   share that checkout and `git checkout --detach` there would yank another lane's tree. Same
   deviation 30-02 and 30-03 made, for the same reason.
3. **`30-02-SUMMARY.md` edited** to close `F-30-03-001`. The plan's scope fence freezes
   `evidence/30-01|02|03` and the source modules — **not** the SUMMARY markdown. The dispatch
   brief explicitly assigns this fix. Prose only; no measurement changed; the INPUTS-FROZEN gate
   re-run after the edit reads **0 dirty**.
4. **Four gate expressions corrected before running** — the MET-forcing `sed` field, the
   `--root`/`--repo-root` argument, the ref-count expression, and the audit's remote-tracking
   premise. Each is filed as a finding rather than silently fixed, because correcting a gate in
   the pass that runs it is the shape of self-grading this phase exists to refuse.
5. **`init-root` also mints the demonstration approval.** The plan's positive-control gate runs
   `init-root` then `verify --approval $D/frontier-positioning.approval.json` with no intervening
   record step, so the file must exist after `init-root`. The key id embeds
   `throwaway-not-seans-key-` and the accepted result carries `root_kind` forward, so the artifact
   announces its own provenance.
6. **Two extra audit determinations (10, not 8)** — AUTH-07 remote-main observability and AUTH-10
   seed non-disclosure. Both strictly add evidence.
7. **`cargo fmt --all` in write mode on the Mac**, not only `--check`. rustfmt performs no
   compilation and the repo's history carries rustfmt-only commits from 30-02 and 30-03.

## 10. Residuals filed

- **HIGH, carried with owners:** `F-30-01-001` (`PEER-PROBE-2026-07-26` names no openable
  artifact), `F-30-01-002..006` (PORT-\* and REACH-\* materially understate the tree),
  `F-30-03-002` (the truncated-hedge class), and **`F-30-04-001` — the ROADMAP status column
  contradicts the tree for Phases 28 and 29**, whose verdicts both exist on disk. The **criteria
  text is current**; only the progress table is stale, and this verdict graded against the
  artifacts, not the table.
- **MEDIUM/LOW to `.planning/BACKLOG.md`, non-blocking:** six new `BL-F30-*` entries. **The
  severity policy was not tightened at the end of the phase** — that is what turned Phase 20 into
  a 74-plan loop.
- **LOW closed here:** `F-30-03-001`, fixed at source.
- **Seam requests:** `.planning/SEAM-REQUESTS/30.md` — four entries, including one *request to
  not act* (do not silently substitute a narrower security extraction) and `SR-30-3`, the only
  unblocking item in the whole register that needs no credential from anybody.

## 11. LIM-18 re-checked rather than inherited

30-03 recorded `LIM-18` with evidence `remedy_under_test_by_another_lane_result_not_in`. **That
snapshot is superseded.** `.planning/HEADLESS-KEYRING-FINDING.md` merged at `769d98b3` and RC item
7 closed at `2a306ac8`: the advertised remedy was **dead in three independent ways**, measured
over 11 live routes, and was fixed (`eabb6ec0`) and re-proven. **`LIM-18`'s substitution point is
discharged.** Recorded here; 30-03's published document was not edited, because a published
limitation is a frozen input.

## 12. What this plan did NOT do

- **It did not position.** No recommendation, no readiness statement, no market comparison. The
  packet says so at the top and its index is gate-checked free of readiness vocabulary — **0**
  hits.
- **No requirement marked complete.** All five F30 closure positions are *stated* in the verdict
  rather than written into the traceability table from inside the phase being graded.
- **No reserved action.** No merge to main, no PR, no tag, no release, no deployment, no issue
  closure, no deletion of a retained evidence ref, no `wcore-contract generate`. The lane branch
  was committed and pushed, which is not a reserved action.
- **No credential.** Sean's approval key was never obtained, requested or simulated. No gate here
  requires a secret and none can be passed by supplying one.
- **Nothing unproven was laundered into proven.** `security ×3` remain UNPROVEN with the meter
  seam as their substitution point; `peer_delta` remains **measured**-UNPROVEN on 148/148; the
  confounded legs remain confounded.
- **No test weakened.** No `#[ignore]`, no `#[allow]`, no re-gating, no deletion, no raised
  timeout. Where an inline test failed, the **oracle** was corrected because the code was right.
- **It did not assert "zero known defects."** Amendment A3 binds and §10 lists what is open.

## Self-Check: PASSED

All created files confirmed present on disk. All commit hashes confirmed via
`git log --oneline --all`. Every number in this summary was read back from a captured run, and
every capture was byte-counted rather than eyeballed.
