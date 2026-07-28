# 28-drift NOTES — running log (append-only, committed as measured)

Lane `lane/28-drift`, base `ef1d97be` (`plan/f20-unified-audit-repair`), started 2026-07-29.

## Measured so far

### M1 — candidate commits bound by BOTH receipts (identical in both)

`bindings.candidate[]`, verified by parsing the JSON, not by grep:

| scope | commit | tree | ledger_ref |
|---|---|---|---|
| `matrix-linux-windows` | `32e2f57d09fe4b287e513081862217dc9daa5901` | `63ec0e6c…` | `evidence/28-02/candidate.json` |
| `matrix-macos-rerun-and-soak` | `e4a3f5fc0f92a7b0126f594146c4b71182e9e378` | `6a494c99…` | `evidence/28-03/candidate.json` |

The superseding receipt's candidate block is **byte-for-byte the same** as the original's. The
supersession changed the disposition of `F-28-02-002`; it did not re-bind the code.

### M2 — drift from HEAD (VERIFIED, matches the inventory exactly)

`/usr/bin/git`, never rtk.

```
32e2f57d  ancestor-of-HEAD: YES   454 commits behind HEAD total
                                  194 commits behind under crates/
                                   19 commits behind under crates/wcore-sandbox/
          authored 2026-07-27T20:11:03+07:00
e4a3f5fc  ancestor-of-HEAD: YES   371 commits behind HEAD total
                                  147 commits behind under crates/
                                   19 commits behind under crates/wcore-sandbox/
          authored 2026-07-28T07:12:12+07:00
```

194 / 147 under `crates/` — the inventory's figures reproduce exactly.

### M3 — six sandbox source files changed (VERIFIED)

`git diff --stat <candidate> HEAD -- crates/wcore-sandbox/src` — **identical file set from both
candidates**, 874 insertions / 69 deletions:

```
backends/appcontainer/acl_lease.rs                 259 +++++---
backends/appcontainer/acl_lease/storage.rs         151 ++++-
backends/appcontainer/acl_lease/tests.rs           387 +++++++++
backends/appcontainer/windows_impl/command.rs       39 ++-
backends/appcontainer/windows_impl/process.rs       48 ++-
backends/appcontainer/windows_impl/tests.rs         59 ++++
```

### M4 — the three commits, and what they actually are (VERIFIED)

```
15821c03  2026-07-29T00:00:15  fix(sandbox): reclaim stale AppContainer ACL leases instead of wedging
          body opens "F-28-02-002." — this IS the repair of the finding whose
          FIXED disposition flipped the acceptance gate to true
3f3f93dc  2026-07-29T00:28:56  fix(sandbox): extract the reclamation report so its wording stays pinned
9c4d2612  2026-07-29T01:39:02  style(sandbox): rustfmt the 0-byte lease tests
```

All three post-date **both** candidates (2026-07-27 and 2026-07-28). So the certified binaries were
built from trees that **do not contain** the fix whose disposition the supersession relies on.

### M5 — F-28-ADJ rows absent from the signed ledger (VERIFIED)

```
evidence/28-04/findings.tsv   sha256 51ddac033dc99a4b1b4d06d3b247b2a4287362b2aae12a9fb83f9513a243e75a
                              74 raw lines, 73 non-blank (10 comment lines + 63 finding rows)
awk -F'\t' '$1 ~ /^F-28-ADJ/'   ->  0 rows
grep -c 'F-28-ADJ'              ->  1  (a mention inside another row's prose)
```

§7 of `28-RECEIPT-SUPERSESSION.md` predicted the receipt would go stale *if* `28-adj2` added these
as rows. It did not add them. So the receipt is not stale by that route — and the ledger is short
two real findings.

### M6 — both verifiers reproduce, on my machine, at my HEAD (VERIFIED)

```
f28-ledger.py --validate evidence/28-04/findings.tsv        rc=0  63 findings, allow_open=False
f28-verify-bindings.py --verify SUPERSEDING-001.json        rc=0  OK
f28-verify-bindings.py --verify <original>.json             rc=1  REJECTED (4)
```

The passing gate is real. It is not a mis-read and it is not vacuous.

### M7 — NO instrument in the toolchain can see drift from HEAD (VERIFIED, and this is the finding)

`f28-verify-bindings.py` checks `bindings.candidate[].commit` against
`evidence/28-0{2,3}/candidate.json` — the ledger the candidate was *resolved from*. That is an
internal-consistency check. It never compares the candidate to any current tree.

```
grep -cE 'rev-parse|rev-list|merge-base|HEAD' over .planning/scripts/f28-*.py
  f28-build-receipt.py      0
  f28-check-matrix-results.py 0
  f28-check-soak.py         0
  f28-ledger.py            30   <- ALL are the string `_LEDGER_HEADER`; zero are git
  f28-resolve-candidate.py  1   <- a self-test fixture string "HEAD^{tree}"
  f28-verify-bindings.py    0
```

Zero git invocations. **The certification cannot go stale in the eyes of its own toolchain, by
construction.**

### M8 — the asymmetry, MEASURED both ways (this is the headline)

The verifier recomputes `fixture_corpus[].sha256` against the **live working tree**
(`f28-verify-bindings.py:482-496`). So it is sharply sensitive to drift in the *test harness* and
totally blind to drift in the *product under test*.

| Perturbation | size | `--verify` |
|---|---|---|
| append one comment line to `crates/wcore-eval-scenarios/src/e5_cases.rs` | 29 bytes | **rc=1**, `F28V-CORPUS` |
| 194 commits under `crates/`, incl. the `F-28-02-002` repair the gate depends on | 874 insertions in `wcore-sandbox` alone | **rc=0**, OK |

Restored byte-exactly afterwards (`3817dc77…`, `git status --short` empty, `--verify` rc=0 again).

**A 29-byte edit to the ruler fails the gate; 194 commits to the thing being measured do not.**

### M9 — the fixture corpus post-dates BOTH candidates (checked, and NOT a defect)

`git cat-file -e` at each commit:

| file | `32e2f57d` | `e4a3f5fc` | HEAD |
|---|---|---|---|
| `e5_cases.rs` | ABSENT | 26718 B | 26718 B (same) |
| `e5_soak.rs` | ABSENT | ABSENT | 26684 B |

Added at `7c54e030` (2026-07-27T21:42) and `1aaeaa2a` (2026-07-28T07:51) respectively — after the
candidate each one tests. **This is correct, not a defect:** E5 is a black-box probe harness that
runs *against* an already-built candidate binary, so it necessarily post-dates it. Recorded so a
later reader does not mistake it for one.

## Still to establish

- [ ] Re-run both verifiers myself (`f28-verify-bindings.py`, `f28-ledger.py --validate`) and read
      the counts back; prove each can still say no.
- [ ] Read `28-ADJ2-SUMMARY.md` for the severity its finder gave `F-28-ADJ-001`/`-002` and the fix
      evidence, so the new ledger rows carry the finder's severity, not mine.
- [ ] Add the two rows; re-issue via `f28-build-receipt.py --supersede` (-002).
- [ ] Cost out re-certification at HEAD; decide whether it should happen at all on a moving branch.
