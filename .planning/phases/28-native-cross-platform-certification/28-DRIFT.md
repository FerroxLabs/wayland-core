# 28-DRIFT — the certification passes, and it certifies code we no longer ship

**Lane:** `lane/28-drift` · **Date:** 2026-07-29 · **Base:** `ef1d97be`

---

## 0. The answer to the question that matters, first

**Is Phase 28's passing gate meaningful today?**

**It passes and it is stale. Both are true and neither cancels the other.**

That is not a hedge, it is the measurement. The gate is a real gate: 65 findings, every one
terminally dispositioned, recomputed from raw evidence by an independent verifier that was
proven able to reject (`--verify` rc=0 on the current receipt, rc=1 on both prior ones, naming
the disagreeing fields). It is not vacuous and it is not self-passing.

And the code it certifies is 194 and 147 commits behind what the project now carries, in a
window that contains **the repair of the very finding whose closure flipped that gate to
passing**. So the sentence "Phase 28's acceptance gate passes" is true, and the sentence
"Phase 28 certifies the code we ship" is false, and until this lane nothing in the record
distinguished them.

**A release cut today would carry a passing certification that never saw the shipped binary.**

I have not resolved that tension by re-scoping the claim in either direction. I made it
impossible to read the gate as covering HEAD (§3), and I have said what re-certification costs
and why I think it should not happen against a moving branch (§6).

---

## 1. The two claims, verified independently before acting

Inventory findings are claims like any other. Both reproduce exactly.

### 1a. The drift

`/usr/bin/git` throughout — `rtk` filters `git log`.

| candidate | scope | ancestor of HEAD | behind, total | **behind under `crates/`** | authored |
|---|---|---|---|---|---|
| `32e2f57d` | `matrix-linux-windows` | yes | 454 | **194** | 2026-07-27T20:11 |
| `e4a3f5fc` | `matrix-macos-rerun-and-soak` | yes | 371 | **147** | 2026-07-28T07:12 |

Both receipts bind the *identical* candidate block — the supersession changed a disposition, it
did not re-bind the code.

Six files under `crates/wcore-sandbox/src` changed in that window, the **same six from either
candidate**, 874 insertions / 69 deletions:
`acl_lease.rs`, `acl_lease/storage.rs`, `acl_lease/tests.rs`, `windows_impl/command.rs`,
`windows_impl/process.rs`, `windows_impl/tests.rs`.

And the three commits are what the inventory said they were:

```
15821c03  2026-07-29T00:00  fix(sandbox): reclaim stale AppContainer ACL leases instead of wedging
                            body opens "F-28-02-002."
3f3f93dc  2026-07-29T00:28  fix(sandbox): extract the reclamation report so its wording stays pinned
9c4d2612  2026-07-29T01:39  style(sandbox): rustfmt the 0-byte lease tests
```

All three post-date **both** candidates.

### 1b. The absent ledger rows

```
evidence/28-04/findings.tsv   sha256 51ddac03…  63 rows
awk -F'\t' '$1 ~ /^F-28-ADJ/'   ->  0
grep -c 'F-28-ADJ'              ->  1   (a mention inside another row's prose)
```

`28-RECEIPT-SUPERSESSION.md` §7 predicted the receipt would go stale *if* `28-adj2` added these
as rows. `28-adj2` **fixed both and added neither**. The receipt dodged staleness by accident and
the signed ledger was short two real findings.

---

## 2. What I found that the brief did not ask for, and which is worse

### 2a. The scope caveat was written down once, at the source, and did not survive one hop

`28-02-SUMMARY.md:40-41`, in bold, by the plan that ran the matrix:

> The integration tip moved from `78b91444` to `c6766f02` while this plan ran, and is still
> moving. **This certification therefore covers `32e2f57d` and NOT the current tip**, and plan
> 28-04 must not read it as covering the tip.

Searched every downstream artifact for that statement or anything equivalent:

| artifact | says which candidate | says how far behind the tip |
|---|---|---|
| `28-04-PHASE-VERDICT.md` | yes (`:99`, `:161`) | **no** |
| `28-04-SUMMARY.md` | yes (`:74`) | **no** |
| `28-04-FINDING-LEDGER.md` / `F-28-04-004` | yes (`:547`) | **no** |
| `28-ADJUDICATION.md` | no mention | **no** |
| `28-RECEIPT-SUPERSESSION.md` | no mention | **no** |
| either signed receipt | binds the commits | **no** |

`F-28-04-004` calls the two-candidate split "the single most important thing a reader of this
receipt must know". It records **which** commits, never **how far behind** either one is. The
only word "drift" anywhere in 28-04 refers to soak latency bands.

So this is not "nobody knew". The plan that took the measurement said it plainly, and the
statement was dropped at every subsequent hop, including by two lanes that were re-examining the
record specifically for defects. **A caveat that lives only in the prose of the plan that made it
is a caveat that will be lost.** That is why §3 puts it in machine-checkable form.

### 2b. No instrument in the toolchain could ever have seen it

`f28-verify-bindings.py` checks `bindings.candidate[].commit` against
`evidence/28-0{2,3}/candidate.json` — the ledger it was *resolved from*. Internal consistency.
Nothing compares the certified commit to any current tree:

```
grep -cE 'rev-parse|rev-list|merge-base|HEAD' over .planning/scripts/f28-*.py
  f28-build-receipt.py         0
  f28-check-matrix-results.py  0
  f28-check-soak.py            0
  f28-ledger.py               30   <- all are the string _LEDGER_HEADER; zero are git
  f28-resolve-candidate.py     1   <- a self-test fixture string "HEAD^{tree}"
  f28-verify-bindings.py       0
```

**Zero git invocations. A Phase 28 certification could not go stale in the eyes of its own
toolchain, by construction.**

### 2c. The asymmetry, measured in both directions — this is the sharpest form of it

The verifier *does* recompute `fixture_corpus[].sha256` against the **live working tree**
(`f28-verify-bindings.py:482-496`). So it is acutely sensitive to drift in the **test harness**
and totally blind to drift in the **product under test**.

| perturbation | size | `--verify` |
|---|---|---|
| append one comment line to `crates/wcore-eval-scenarios/src/e5_cases.rs` (harness) | 29 bytes | **rc=1**, `F28V-CORPUS` |
| 194 commits under `crates/`, incl. the `F-28-02-002` repair the gate depends on | 874 insertions in `wcore-sandbox` alone | **rc=0**, OK |

Restored byte-exactly afterwards (`3817dc77…`, `git status --short` empty, `--verify` rc=0 again).

**A 29-byte edit to the ruler fails the gate. 194 commits to the thing being measured do not.**

### 2d. The repair has never been through the matrix, on any platform

Checked, not assumed. Every `candidate.json` / `results.json` / `soak.json` in the phase names
only `32e2f57d` or `e4a3f5fc`. Both predate `15821c03`. So the AppContainer ACL-lease repair —
the code the passing gate now depends on — has been proved by unit tests and **one manual live
reproduction on a single Windows host**, and by no certification-matrix cell anywhere. This is
disclosed inside the signed body of the new receipt, and it is the reason §6's answer carries a
precondition rather than being a simple "wait".

---

## 3. Recording the drift where a consumer actually looks

A certification whose scope is silently narrower than a reader assumes is the same failure class
as a gate that passes because it checks nothing. Prose alone would repeat §2a. So the drift is
recorded in four places, three of them machine-checkable.

### 3a. A new instrument — `.planning/scripts/f28-check-drift.py`

Compares every bound candidate against a given ref and reports commit distance under `crates/`.
Deliberately **not** an acceptance gate: staleness is a fact about scope, not a defect.

```
$ f28-check-drift.py --receipt <any of the three receipts> --ref HEAD          rc=1
  matrix-linux-windows:        STALE commit=32e2f57d scoped_behind=194 files_changed=199
  matrix-macos-rerun-and-soak: STALE commit=e4a3f5fc scoped_behind=147 files_changed=149
  F28D-003 … The certification is VALID for that commit and STALE for this ref;
            both are true and neither cancels the other.
```

**`--self-test` carries three assertions, not two** — known-negative (a candidate equal to the
ref is not reported stale), known-positive (the real receipt is, with code `F28D-003`
specifically), **and that `f28-verify-bindings.py --verify` returns 0 on the very receipt this
reports stale**. Without that third one the self-test would pass just as happily on an instrument
that adds nothing.

`probe-drift-codes.sh` fires the other four codes — `F28D-000` bad ref, `F28D-001` unknown
commit, `F28D-002` diverged rather than aged, `F28D-004` no candidate bound — each from a fixture
built to trip exactly it, plus a pristine control measured against the certified commit itself
(`matrix-linux-windows: CURRENT scoped_behind=0`). A checker that rejected everything would pass
all four probes and be worthless. `PROBE_RESULT=PASS`.

### 3b. A scope warning in the ledger header

Fourteen comment lines at the top of `evidence/28-04/findings.tsv`, above the field header, where
anyone reading a `FIXED` row will see them. It names the two commits, the 194/147 figures, the
three repair commits, and the command to re-measure rather than trust the comment.

### 3c. Inside the signature

`28-04-CERTIFICATION-RECEIPT-SUPERSEDING-002.json` carries the drift as a `posture` binding,
covered by `body_sha256`, not in a sidecar nobody digests:

- `certified-code-is-STALE-relative-to-the-integration-branch` — the figures, the repair commits,
  and the sentence *"This receipt does NOT certify the integration branch, does NOT certify any
  release candidate, and must not be read as covering either."*
- `no-checker-in-this-toolchain-could-see-that-staleness` — §2b and §2c, with the both-directions
  measurement.
- `known-open-findings-outside-this-ledger` — §5.

### 3d. This document

---

## 4. Completing the signed ledger

`F-28-ADJ-001` and `F-28-ADJ-002` are now rows, at **MEDIUM** — the severity their finder gave
them in `28-ADJUDICATION.md` §6, not one I chose — carrying lane `28-adj2`'s fix evidence:
the 8/8 pre-resolved test list, `136 passed / 0 failed / 23 ignored` (up from 133/0/23), and
mutants M3–M6 each killing exactly its target. 63 → 65 rows.

Both rows state, in their own rationale, that the repair landed at `9c4d2612`, which post-dates
both certified candidates, so they are FIXED on the integration branch and **not present in the
certified binaries**.

**The strongest confirmation is one I did not write.** `f28-verify-bindings.py
--check-enumeration`, run against the receipt that disclosed these two only in prose:

```
F28V-ENUM  F-28-ADJ-001: FIXED in the ledger and ABSENT from the signed receipt
F28V-ENUM  F-28-ADJ-002: FIXED in the ledger and ABSENT from the signed receipt
```

That is the incompleteness argument, restated by the tool, in reverse.

**Proof the ledger gate still bites on the rows I added** — four tampers on `F-28-ADJ-001`, each
targeting a different rule, plus the pristine control (`probe-ledger-tampers.sh`):

| tamper | rc | codes |
|---|---|---|
| `disposition=OPEN` | 1 | `F28L-002` |
| `executable_check` emptied | 1 | `F28L-008` |
| `ACCEPTED` + contradicts criterion 2 | 1 | `F28L-004 F28L-005 F28L-007` |
| `p28_severity=SEVERE` | 1 | `F28L-011` |
| pristine control | **0** | — |

---

## 5. The re-issued receipt

`28-04-CERTIFICATION-RECEIPT-SUPERSEDING-002.json`, built with
`f28-build-receipt.py --supersede …-SUPERSEDING-001.json`. Nothing writes to the original or to
`-001`; both remain byte-identical.

| | original | `-001` | **`-002`** |
|---|---|---|---|
| `body_sha256` | `2037352c…` | `8db1ef07…` | **`bdfc5026…`** |
| findings | 63 | 63 | **65** |
| `F-28-02-002` | OPEN | FIXED | FIXED |
| `F-28-ADJ-001/-002` | absent | prose only | **ledger rows** |
| drift disclosed | no | no | **yes, in the signed body** |
| acceptance gate | false | true | **true** |
| `--verify` today | rc=1 (4) | rc=1 (2) | **rc=0** |
| `--check-enumeration` | rc=1 (3) | rc=1 (2) | **rc=0** |

**Amendment A3 is honoured, and it binds harder here, not less.** Three true claims are not
"zero known defects", so four defects outside the ledger are named inside the signature:

1. a **partial** lease write (power loss mid-write) leaves a non-empty unparseable lease that
   still wedges — `28-adj2` named it and deliberately did not fix it, because a non-empty
   unreadable lease is not safely distinguishable from a tampered one;
2. the accounting control has **no consumer** — Phase 29 binds `receipt_body_sha256` and
   `receipt_signing_key_id` and never reads `body.findings`, the failure `28-04-SUMMARY.md:310`
   predicted for itself;
3. three `required_live_bwrap_*` live-acceptance tests fail on Windows, environmental and
   pre-existing, reported rather than silenced;
4. **§2d** — no matrix run has ever exercised the `F-28-02-002` repair.

**Seam consequence for the orchestrator:** Phase 29 pins `receipt_body_sha256` and
`receipt_signing_key_id`. Any future release manifest must pin **`-002`'s** pair
(`bdfc5026…` / `phase-28-certification-supersession-002-2026-07-29`), not `-001`'s and not the
original's. Not something to act on unilaterally here.

---

## 6. What re-certification at HEAD would cost, and why I say do not do it

Measured from the artifacts of the runs that actually happened, not estimated.

| leg | volume | host | reachable from |
|---|---|---|---|
| matrix, linux | 216 cells | `hetzner-dsm` | anywhere |
| matrix, macOS | 216 cells (+216 re-run at 28-03) | certification Mac, macOS 26.3 arm64 | Sean's machine |
| matrix, windows | 219 cells | `seandesktop` | **Sean's Mac only** — `hetzner-dsm` cannot reach it |
| soak | 3 families × 1000 sessions × concurrency 4 | all three | — |
| | 651 matrix cells, 147 critical; 3000 soak sessions | 3 physical hosts | |

Both prior plans recorded `duration: ~1 session` each, so the floor is **two full sessions of
operator-attended work across three machines**, one of which only one person can reach.

**But wall-clock is not the binding constraint, and treating it as one gets the answer wrong.**

**A certifiable candidate must be a commit with all six per-target CI release artifacts.** The
matrix runs the *CI release artifact itself*, digest-asserted on each host before the run —
`28-01` bound **0 of 6** purely because the CI run was still `queued`, and `28-02` had to
re-resolve the candidate once it completed. The integration branch does not produce a
six-artifact release build per commit. So "re-certify at HEAD" is not a matter of spending
compute: **it requires first freezing a commit and driving a full release build at it.** Freezing
a commit and building it is what designating a release candidate *is*.

### The decision

**Certify at a designated, frozen release candidate. Do not re-certify against the integration
branch, and until an RC exists the record must say the certification is stale.**

Re-running against a branch that merges from five lanes hourly manufactures another receipt that
is stale on arrival — more of the exact artifact this lane exists to repair. A certification only
binds meaningfully to an immutable target.

**One precondition, and it is not optional.** §2d is a live coverage hole that "wait for the RC"
would otherwise bury indefinitely: the `F-28-02-002` repair has never been through the matrix on
any platform. Before any RC certification is trusted, **the Windows sandbox cells must run at a
commit containing `15821c03`.** That is one family on one host, not a three-platform
re-certification, and it can be scheduled independently of the RC. Until it runs, the strongest
honest statement about that repair is "unit-tested and reproduced once by hand", which is *not*
what a certification asserts.

### Cross-audit

Question put to all three panelists with the five verified facts stated. Position token extracted
unanchored; codex read from the **last** match.

| auditor | position |
|---|---|
| `codex` gpt-5.6-sol | `CERTIFY_AT_RC` — *"mechanically valid for its evidence record, but not meaningful as certification of today's integration branch… retain the existing receipt for audit history but mark it explicitly stale and non-authoritative"* |
| `gemini` 3.1-pro | `CERTIFY_AT_RC` — *"the receipt mathematically binds to software that still contains the failing condition… chasing HEAD guarantees the certification will be stale again within hours"* |
| `kimi` K3 | `CERTIFY_AT_RC` — *"rc=0 verifies internal consistency, not current safety… certification only binds meaningfully to an immutable target"* |
| internal adversarial | argued **`RECERTIFY_NOW`**: `CERTIFY_AT_RC` is how a certification becomes decorative. No RC is designated or scheduled, the fix the gate depends on has never seen the matrix, and "passing, with a footnote" is what readers actually take away. |

**The adversary lost on direction and won on substance, and both halves are in the answer.** It
lost because the binding constraint is the six-artifact CI release build and the single-operator
Windows path, neither of which is bought with compute — and because re-certifying a moving branch
produces the defect being repaired. It won on the specific hole it named, which is why the
recommendation carries the §2d precondition instead of a plain "wait".

**Codex silently dropped its vote on the first invocation** — backgrounding it detached stdin
(`Failed to read prompt from stdin`, 131 bytes, rc=1). Re-invoked in the foreground; the recorded
vote is that run. This is the fourth time on this program that a panelist's silence has been
mistaken for nothing, and the byte count is what caught it.

---

## 7. Two defects in my own instruments, found by reading output and repaired here

§6b-ii: a written-up instrument defect is a defect you have agreed to keep.

**1. `f28-check-drift.py --self-test` pinned its fixture to `SUPERSEDING-001`.** The moment
`-002` was issued, `-001`'s ledger digest went stale *by design* and assertion 3 began reporting
a failure that was not one. A self-test pinned to a superseded artifact measures history.
Repaired to select the highest-numbered supersession, and the fixture name is now printed so a
reader sees what was measured. **Proven the repair does something:** the pre-repair file
(`git show ca271612:…`) run from the correct path → **rc=1**; repaired → **rc=0, fixture =
SUPERSEDING-002**.

**2. My first gate harness silently ran nothing.** It built `"--validate $L"` as one string and
passed it unquoted. In bash that word-splits and works. **This shell is zsh, which does not
word-split unquoted parameter expansions**, so argparse received one argv entry, and three gates
returned `rc=2` while looking run. Repaired to explicit argv arrays in `gates.sh`, plus a
**harness self-test**: a known-good gate must return 0 first, or every exit code below it is
noise. Adding to the program's running list, this is a sixth flavour of the self-passing shape —
an invocation that never reached the tool.

**3. The Rust supersession test covered only the first link of the chain.** It named
`SUPERSEDING-001` as a constant and paired it against the *original*. Correct while the chain had
one link; the moment `-002` existed it covered nothing, because `-002` supersedes `-001`, not the
original. Generalised to discover every `SUPERSEDING-*.json` and resolve each one's predecessor
from its **own** artifacts binding. The flipped-byte test carried the sharper version of the same
defect — it hardcoded `phase-28-certification-2026-07-28`, the *original's* key id, which appears
nowhere in `-002`; `replacen` would have found nothing and the failure would have surfaced on
"the mutation must actually mutate", a confusing assertion standing in for absent coverage.

---

## 8. The gates, and the one that went red

**Rust, `hetzner-dsm`, by file never by filter, executed count read back rather than inferred.**

The generalised test's **first act was to fail on my own receipt**:

```
thread 'the_superseding_receipt_verifies_and_names_what_it_supersedes' panicked:
  a receipt asserting all three claims true must disclose known findings that are
  outside its ledger, or three trues read as 'zero known defects'
test result: FAILED. 27 passed; 1 failed; 0 ignored; 0 filtered out
```

The A3 contract requires a `posture` whose name contains `known-open-findings`. I had named mine
`known-defects-outside-this-ledger`. The disclosure was present; its *contract name* was not, and
the contract is what a machine can check. **Not weakened — the receipt was rebuilt to satisfy
it.**

**Proof the repair does something, and this is the cleanest one in the lane:** the pre-change
test file, on the **identical tree**, passes **28 / 0**. It only ever looked at `-001`. The defect
was invisible to it.

```
OLD test file, tree 708ad83d:  test result: ok. 28 passed; 0 failed  (WLRC=0)
NEW test file, tree 3d14c285:  test result: FAILED. 27 passed; 1 failed  (WLRC=101)
NEW test file, tree 708ad83d:  test result: ok. 28 passed; 0 failed  (WLRC=0)
```

Non-vacuity from `--nocapture` — both links executed, not early-returned:

```
supersession chain: 2 link(s)
verified 63 findings, gate_passed=false, unresolved CRITICAL/HIGH = ["F-28-02-002"]
superseding receipt verified: …SUPERSEDING-001.json (63 findings, gate_passed=true)
  supersedes …RECEIPT.json (2037352c…) under phase-28-certification-2026-07-28
superseding receipt verified: …SUPERSEDING-002.json (65 findings, gate_passed=true)
  supersedes …SUPERSEDING-001.json (8db1ef07…) under phase-28-certification-supersession-001-2026-07-29
```

`-002` was **rebuilt rather than superseded by a `-003`**, deliberately and stated plainly: the
generalised test walks the whole chain, so a defective link stays red forever no matter what
supersedes it. `-002` had not entered the record — it lived ~40 minutes on an unmerged lane
branch and never verified under the Rust half. The original and `-001` were not touched.

**Python, all green, full transcript in `evidence/28-drift/gate-transcript.txt`:**

```
f28-ledger.py         --self-test 0 · --validate 0 (65, allow_open=False) · --check-a2 0
                      --check-downgrades 0 · --check-backlog-ids 0 (46 paper rows)
                      LEDGER_TAMPER_RESULT=PASS  (4 tampers red, control green)
f28-verify-bindings   --self-test 0 · --check-requirements 0
                      original  --verify 1 (4) · --check-enumeration 1 (3)
                      -001      --verify 1 (2) · --check-enumeration 1 (2)
                      -002      --verify 0     · --check-enumeration 0
                      --check-tamper-detection 0 and --check-claim-limit 0 on all three
f28-check-drift.py    --self-test 0 (3 assertions) · all three receipts rc=1 STALE
                      PROBE_RESULT=PASS  (F28D-000/001/002/004 each fired; control CURRENT)
cargo fmt --all -- --check   rc=0
```

`--verify` failing on the original and on `-001` is correct, not a regression: a signed receipt
cannot track a moving ledger, which is the argument for supersession restated as a measurement.

---

## 9. Honest limits

- **The Rust leg ran on `hetzner-dsm` only.** No macOS or Windows execution. This lane changed
  documents, Python scripts and one test file; nothing platform-specific. Live-exercising the
  `wayland-core` binary would not test anything this lane built.
- **§2d is a static finding.** I did not run the Windows sandbox cells at `15821c03` — that is
  the precondition §6 asks to be scheduled, and it needs `seandesktop`, reachable only from
  Sean's Mac. I did not attempt it and I am not claiming it is impossible; it is out of this
  lane's scope and I am naming it rather than absorbing it.
- **The drift figures move.** `scoped_behind` under `crates/` was 194/147 throughout; the
  *overall* counts rose from 454/371 to 457/374 as my own commits landed. Anyone re-measuring
  will get larger overall numbers and should. The command is in the ledger header.
- **`f28-check-drift.py` is not wired into any acceptance gate**, deliberately. Staleness is a
  scope fact, not a defect, and making it fail acceptance would pressure a future lane to
  re-certify a moving branch — the thing §6 argues against.
- **The `posture` prose is not machine-checked by the Python verifier** — `posture` is free text
  to `f28-verify-bindings.py`. The Rust half checks the supersession digest and key id against
  the file on disk; the drift text itself is covered only by `body_sha256`.
- I did **not** merge, open a PR, tag, release, close an issue, run `wcore-contract generate`,
  or supply a credential. No shared-fence file (`wcore-cli/src/lib.rs`, `main.rs`) was touched.
