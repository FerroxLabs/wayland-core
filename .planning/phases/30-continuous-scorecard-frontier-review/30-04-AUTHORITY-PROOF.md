# 30-04 — Authority proof: both runs, and the audit that this lane took no reserved action

**Run at** `5d48ec270f642498cc095bd21ad4a256e628c4c3`, lane `lane/30-04`, on `hetzner-dsm`
(Linux). Binary: `target/release/wayland-scorecard`, built `--release --locked -p
wcore-eval-scenarios`, rc=0.

Machine-readable index: `evidence/30-04/authority-audit.tsv` — **10 determinations, 10
well-formed of 10 total**, every one naming a capture under `evidence/30-04/captures/` that
exists and holds real output.

---

## 0. The two sentences, stated plainly and together

**The mechanism works.** A throwaway approval root generated at run time ACCEPTS a valid
frontier-positioning approval. Exit status 0, captured.

**No approval exists in this lane.** The same approval, verified against the committed bundled
placeholder root, is REFUSED with an explicit error naming its own substitution point.

Reporting either of these without the other is precisely how a lane would drift into acting as
though it had approval. The first alone reads as "positioning is reachable"; the second alone is
satisfied by a verifier that refuses everything and proves nothing about whether an approval can
ever be honoured. Both were run, in that order, and both are below.

---

## 1. The positive control — run FIRST, deliberately

`evidence/30-04/captures/auth-01-positive-control.txt`, 1391 bytes.

```
AUTHORITY_INIT_ROOT=OK root_kind=throwaway_generated_at_run_time
key_id=throwaway-not-seans-key-31007056bb20fee7 public_key_hex=31007056bb20fee7d394117aca261ac81b592a8fe4551b1921dc26f3c421ebed
root=/tmp/tmp.cxaic9AVXN/root.json approval=/tmp/tmp.cxaic9AVXN/frontier-positioning.approval.json subject_sha256=d89c9063e7f33bdd657b6bc1306bdcc0c1d2a9ac84eb54a05cdf018a88d995b3
NOTE: this root was generated at run time and is NOT Sean's approval key. It exists to prove the mechanism can ACCEPT a valid approval; an acceptance under it authorises nothing.

600 /tmp/tmp.cxaic9AVXN/throwaway-not-seans-key-31007056bb20fee7.seed

AUTHORITY_VERIFY=ACCEPTED action=frontier_positioning principal=sean subject_sha256=d89c9063e7f33bdd657b6bc1306bdcc0c1d2a9ac84eb54a05cdf018a88d995b3 key_id=throwaway-not-seans-key-31007056bb20fee7 root_kind=throwaway_generated_at_run_time
NOTE: root_kind is not operator_supplied. This acceptance proves the MECHANISM works; it is not an approval and authorises nothing.

POSITIVE_CONTROL_RC=0
```

Three properties of this run are load-bearing:

- It drove the **real shipped verifier**, never a test double. The same binary a gate runs.
- The root **declares itself throwaway in its own key id** — `throwaway-not-seans-key-…` — and
  the accepted result **carries the root kind forward**. An acceptance that does not say WHICH
  root honoured it is exactly how a clean-room proof gets quoted as authority later.
- The seed reached a **mode-0600 file and nothing else**. Measured, not asserted: see §4,
  AUTH-10.

## 2. The refusal — structural unreachability

`evidence/30-04/captures/auth-02-placeholder-refusal.txt`, 646 bytes. Remote exit status **0**,
which under this gate's shape means *the refusal happened*.

> `wayland-scorecard: reserved action `frontier_positioning` is unreachable: the approval trust
> root declares key `sean-reserved-approval-root` as the all-zeros placeholder, which authorises
> nothing. Substitution point: replace APPROVAL_ROOT_PUBKEY_HEX in
> crates/wcore-eval-scenarios/src/reserved_authority.rs with Sean's real Ed25519 approval public
> key. Until that substitution is made, every reserved action including frontier positioning is
> structurally unreachable from this repository. (F-030)`

**The gate could not be satisfied by ssh failing.** The naive form — assert a non-zero remote
status — passes when the host is down, when the checkout fails and when the binary is missing,
none of which say anything about a refusal. So the remote side decides: setup failures exit
non-zero, an unexpected acceptance exits **9**, and only the refusal exits **0**. The local
assertion is the ordinary zero.

This copies the shape `IndexVerifier::bundled()` already proves in
`crates/wcore-cli/src/plugin/index.rs` (F-021) rather than improvising a new one. That
precedent is gate-checked intact at its measured baseline: `F-021` appears **6** times and
`INDEX_PUBKEY_HEX` **6** times, exactly the counts recorded at planning.

## 3. What the contract suite proves around those two runs

**14 tests run: 14 passed, 0 failed, 0 ignored** — the executed count read back, never the exit
status. Full crate: **505 run, 505 passed, 5 skipped**.

| Property | Test |
|---|---|
| an invented action fails to deserialize | `an_invented_reserved_action_named_termination_state_4_fails_to_deserialize` |
| no agent principal exists to write | `an_approval_whose_principal_is_the_agent_fails_to_deserialize` |
| no self-approval principal exists | `a_self_approved_principal_fails_to_deserialize` |
| one action's approval is not another's | `an_approval_minted_for_a_source_push_does_not_verify_as_a_release` |
| an approval is bound to its subject | `an_approval_moved_onto_a_different_subject_digest_is_refused` |
| an unknown key id is refused, not trusted | `an_unknown_key_id_is_refused_rather_than_trusted` |
| the placeholder refuses all nine actions | `frontier_positioning_is_refused_under_the_bundled_placeholder_root` |
| **the mechanism can ACCEPT** | `frontier_positioning_verifies_under_a_throwaway_root_generated_at_run_time` |

Every refusal carries a **pristine control accepted first**, and the last row is the mandatory
positive control. Nine actions, **nine distinct signature domains counted distinctly** — nine
repetitions of one domain would not satisfy the gate.

---

## 4. The audit — did this lane take a reserved action?

| ID | Determination | Status |
|---|---|---|
| AUTH-01 | a throwaway root ACCEPTS a valid frontier-positioning approval | CONFIRMED |
| AUTH-02 | the same approval is REFUSED under the committed placeholder root | CONFIRMED |
| AUTH-03 | no local branch named `main` or `master` contains this lane's HEAD | CONFIRMED *(weak — see below)* |
| AUTH-04 | no tag points at this lane's HEAD | CONFIRMED |
| AUTH-05 | the named retained evidence refs still resolve | CONFIRMED |
| AUTH-06 | the retained-ref count has not fallen | CONFIRMED |
| AUTH-07 | this lane's HEAD is **not** contained in `main` **on the GitHub remote** | CONFIRMED |
| AUTH-08 | whether a pull request was opened | **NOT-OBSERVABLE-HERE** |
| AUTH-09 | issue closure, release, deployment, frontier positioning | **NOT-OBSERVABLE-HERE** |
| AUTH-10 | no signing seed reached stdout, stderr or an argv | CONFIRMED |

### AUTH-03 is a weak confirmation and saying so matters

**There is no local `main` or `master` branch in this repository at all** —
`LOCAL_MAIN_EXISTS=0`. So the "no local main contains HEAD" check passes *vacuously*: it would
pass at base, and it would pass even if this lane had merged to main on the remote. The plan
conjoins it with a completion anchor, which proves the task ran but does not make the check
mean more than it does. It is recorded as CONFIRMED because it is true, and flagged as weak
because on its own it would be a self-passing gate. **AUTH-07 is what actually carries this.**

### AUTH-07 — the plan's stated ceiling was WRONG, and the ceiling is narrower

The plan's `read_first` states that *"this repository has NO remote-tracking refs at all, which
is what bounds the audit."* That is **false against the tree**:

```
REMOTE_TRACKING_REFS = 238        (144 gh, 94 origin)
refs/remotes/gh/main     = 61b79c4f90f71fe2cf243affa7620b3c9b607f14
refs/remotes/origin/main = ea3bb1c584b157e7e83d34db25d96e1136e9f584
ls-remote gh refs/heads/main = 61b79c4f90f71fe2cf243affa7620b3c9b607f14   (cached view is CURRENT)
HEAD_IS_IN_REMOTE_MAIN = NO
CONTROL (main tip vs main) = YES  -- the check CAN answer YES, so the NO is a real measurement
```

So the main-merge half of the ceiling **is closable and was closed**, by a read-only
`ls-remote` and an ancestry test with a falsification control. No credential was obtained,
requested, printed or copied to do it: this is the same ambient read path the lane already used
to push its own branch, and a read is not a reserved action.

This is a better outcome than the plan predicted, and it is reported as a **correction to the
plan**, not as a bonus.

### AUTH-08/09 — what genuinely cannot be observed, stated precisely

What remains is everything that **writes no git object at all**. A pull request is GitHub
state; `refs/pull/*` is not fetched here (`REFS_PULL_COUNT=0`). An issue closure, a release
publication, a deployment and a frontier positioning statement leave no ref, no tag and no
commit. **No command in this repository can observe them**, and reading them would need a
GitHub API credential this lane must never hold.

The honest statement, and this lane claims exactly this and no more:

> No reserved action was taken within this repository's reach. The main-merge half is
> **measured**, not merely asserted. The pull-request, issue-closure, release, deployment and
> positioning half is **Sean's to confirm**, and no credential was obtained to close it.

### AUTH-06 — the ref-count gate is weaker than it looks

Measured two ways, both recorded:

| Expression | Count |
|---|---|
| the plan's: `for-each-ref \| grep -cv '^refs/heads/'`, floor 37 | **275** |
| of which remote-tracking | 238 |
| the TIGHT count — tags + `refs/f20a/*`, the refs whose deletion IS the reserved action | **37** |
| planning baseline | **37** |

The tight count reproduces the baseline **exactly**, which is the meaningful result. But note
what the plan's expression does: against an actual 275, a floor of 37 **could not detect the
deletion of 238 refs**. The gate passes, and it passes for the wrong reason. Filed as a MEDIUM
finding rather than silently corrected, because correcting a gate expression in the pass that
runs it is the shape of self-grading this phase exists to refuse.

### AUTH-10 — no seed disclosure, measured live with a control

```
SEED_LEN=44
SEED_LEAKED=NO
CONTROL_GREP=FOUND        -- the identical grep DOES find a string that is in stdout
600 /tmp/.../throwaway-not-seans-key-d1c689467a0a14c6.seed
```

The control matters: without it, `SEED_LEAKED=NO` would also be the answer if `grep` had been
broken or the file empty. The binary additionally declares **no** clap argument whose name
carries `seed`, `private` or `secret` — gate-checked at zero, and red at base.

---

## 5. Instrument defects found while taking these measurements

Recorded rather than quietly corrected, because on this program the instrument that hunts a
defect class keeps turning out to carry it.

1. **My own falsification harness manufactured a self-passing gate.** Checking that the Task-1
   gates go red at base, I materialised the base tree with `git show BASE:path > file`. That
   **creates the file even when `git show` fails**, so a file absent at base appeared as
   present-and-empty and `test -f` passed. Six of seven gates still went red — their `grep` legs
   failed on the empty file — which is exactly what hid it; `NO-SECRET-ON-ARGV` alone reported
   GREEN AT BASE and looked like a defective gate in the plan. Re-run with `git archive | tar
   -x`, **all seven are red**. The gate was sound; my falsifier carried the defect it was
   hunting.

2. **`rtk` silently filtered a `git for-each-ref` grep.** An interactive listing of the
   remote-tracking main/master refs printed **nothing**, while `grep -c` on the identical
   pipeline counted **2**. Same class 30-03 measured on `git log`: not fabricated, *filtered*,
   which is worse because it looks complete. Everything load-bearing here is file-captured and
   byte-counted.

3. **My own anchored regex silently lost every match.** Adding `%(objectname)` to the
   `for-each-ref` format put a SHA after the refname, so the anchor `/(main|master)$` matched
   **zero** lines where the unanchored form matched **two**. This is verbatim the trap the lane
   brief names, and I walked into it while writing the check that was supposed to catch it.

## 6. What this document does NOT establish

- **It does not establish that positioning is warranted.** It establishes that positioning is
  *structurally unreachable here*, which is a statement about this repository's trust root and
  about nothing else.
- **The clean-room acceptance is not an approval.** This lane generated the key it verified
  against. That proves the mechanism, and the plan requires both sentences precisely so the
  proof is never mistaken for the thing it proves is possible.
- **It does not observe GitHub.** See AUTH-08/09.
- **Linux only.** Both runs are `hetzner-dsm`. The module is pure Rust with one `#[cfg(unix)]`
  branch (the 0600 mode); the Windows branch is unexercised and is not claimed.
