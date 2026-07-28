# 28-ADJUDICATION — `F-28-02-002`, independent re-adjudication

**Verdict: FIXED.** Reached by trying to break the claim, not by confirming it.

| | |
|---|---|
| Lane | `lane/28-adj` — did **not** author the repair |
| Base | `b79f141e` (`plan/f20-unified-audit-repair`) |
| Question | does the `FIXED` claim for `F-28-02-002` survive an adversarial pass? |
| Available dispositions | `FIXED` or `DISPROVED` only. `ACCEPTED`/`DEFERRED` closed at HIGH; a downgrade to reopen them is a named forgery and was not taken |
| Hardware | `SeanDesktop` (`ssh SeanD@seandesktop`), repo `C:\f28h2-repo` at `3f3f93dc` |
| Evidence | `evidence/28-adj/` |

---

## 1. The first thing I checked, because the summary says the opposite

`28-H2-SUMMARY.md` §8 states in terms: *"The fix is on `lane/28-h2` only. Not merged, no PR."*
If that were still true, writing `FIXED` into the ledger would be a paper disposition — the
ledger would certify a repair the shipped tree does not contain.

It is **stale, not wrong.** It was true when written; the lane branch was merged afterwards.

```
git merge-base --is-ancestor 15821c03 HEAD   -> ANCESTOR   (the repair)
git merge-base --is-ancestor 3f3f93dc HEAD   -> ANCESTOR   (the report extraction)
git diff 3f3f93dc HEAD -- .../acl_lease.rs .../acl_lease/   -> empty
sha256(acl_lease.rs) = bc6bdac1…  on the Mac at integration HEAD
sha256(acl_lease.rs) = bc6bdac1…  on SeanDesktop at 3f3f93dc
```

`166ce7fe` — the commit whose message announces the fix — is **docs-only**, 20 files all under
`.planning/`. The source landed separately. Anyone auditing by reading commit subjects would
have concluded the opposite of the truth in either direction. What is merged is byte-identical
to what was tested on hardware, so every measurement below is a measurement of the shipped code.

## 2. What I tried to break

### 2.1 The quarantine allow-list — does it create a new wedge, surface, or trust crossing?

This was the most promising attack: the repair removes a permanent refusal by teaching a
hard-erroring scanner to *skip* something, and a skip is where a hole hides.

It survives, and it is narrower than it needed to be.

- **Scope.** `acl_lease.rs:635` skips only `file_type.is_dir() && file_name == "quarantine"`.
  On Windows, std's `FileType::is_dir()` is false for reparse points, so a **junction** planted
  under that name is not skipped — it falls through to the hard-error branch and **fails
  closed**. A plain *file* of that name likewise still hard-errors. The allow-list admits
  exactly one shape.
- **No read-back.** `read_dir` is not recursive and the branch `continue`s. Nothing in product
  code ever opens, parses, validates or trusts anything inside `quarantine/`; the only reader
  in the tree is `tests.rs:268`. Quarantined bytes are inert.
- **No new writable surface.** `create_or_open_child_directory` (`storage.rs:525`) is the *same*
  helper that already builds the `Wayland/Core/AppContainerLeases/v1` chain, with the same
  `CreateDirectoryW(…, NULL)` inherited DACL, the same `open_directory_nofollow` reparse
  rejection, and the same `same_windows_path` post-check. The quarantine directory is exactly
  as privileged as the lease root that has always been there — no delta to exploit.
- **No unbounded growth into a new wedge.** `quarantine_lease` allocates via a monotonic
  `TEMP_COUNTER` plus the pid and moves with `MoveFileExW` *without* `MOVEFILE_REPLACE_EXISTING`,
  so an existing artifact is never clobbered; the "could not allocate a unique name" error is
  unreachable in practice.
- **`recover_rewrite_temps` is unaffected** — it filters on `is_rewrite_temp_name`, which a
  directory named `quarantine` cannot match.

### 2.2 "Dropping the allow-list kills only the re-entrancy test" — the lane's claim

**True, and I found the reason the lane did not state.** `mutate2.ps1` runs each of the four
tests in its **own `cargo test --exact` invocation**, and `test_lease_root` (`storage.rs:98`) is
keyed on `std::process::id()` and wiped at start — so every invocation gets a **fresh lease
root**. Only `quarantine_directory_does_not_become_a_second_wedge` performs two recovery passes
in one process, so only it can observe the directory at all.

This *strengthens* the repair rather than weakening it: in the **full suite** all four tests
share one root, so the allow-list is exercised harder there than the by-name mutation implies.

### 2.3 Honour-when-alive — the leg that would trade a DoS for the opposite break

Survives, and again is stronger than claimed. `acl_lease.rs:652`

```rust
if owner_is_live(&lease)? { continue; }
```

is the **first** statement in the per-lease loop, so it dominates **all three** mutating
branches — `DeleteAppContainerProfile` (`:660`), reclamation (`:680`, `:693`) and
`cleanup_locked` (`:696`) — not merely the reclaim path the summary discusses. A live owner's
lease is untouched on every route through the function.

Mutant `M2` (`if false`) fails `live_owner_unreconcilable_lease_is_honoured_not_reclaimed` and
**nothing else**, so that test is not satisfiable by a reclaim-everything implementation. It is
corroborated at the real-profile level by `live_owner_is_never_reclaimed` passing under
`WAYLAND_SANDBOX_LIVE_WINDOWS=1` against a genuine `CreateAppContainerProfile` identity.

### 2.4 Does reclaiming trade a denial of service for a containment hole?

No, and the direction is the reverse of the worry.

| | before | after |
|---|---|---|
| sandbox | permanently unavailable | available |
| product | refuses **all** execution | executes **sandboxed** |
| stale ACEs | remain, undisclosed | remain, disclosed |

The residual ACE is granted to a SID the lease stores only as a digest. AppContainer SIDs derive
deterministically from the profile *name* — but "unreconcilable" is *by definition* the case
where the recorded SID does **not** match the SID derived from the recorded profile name, so the
grant's SID cannot be reached from anything the lease discloses. And the prior behaviour never
revoked those ACEs either: its documented remedy was *delete the file by hand*, which leaves the
identical residual while also keeping the sandbox off. Quarantining strictly dominates.

## 3. The fourth self-passing gate — assumed, hunted, and MEASURED

`F-28-ADJ-001`. The brief said to assume a fourth existed that the lane did not catch. It does,
and it is in the lane's own instrument set.

**The test named `reclamation_reports_grants_it_could_not_revoke` does not test that.** It never
calls `reclamation_report`; the only occurrence of that identifier anywhere in `tests.rs` is the
test's own *name* (`tests.rs:359`). Its assertion reads the **quarantined file** and checks it
still contains the grant path — which is satisfied by the move preserving file contents, and is
already asserted by `dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever`. The only
test that does call `reclamation_report` (`storage.rs:1067`) passes `test_lease(…)`, whose
`intents` are `Vec::new()` (`storage.rs:776`) — so only the `if` branch is ever covered, and the
`else` branch at `acl_lease.rs:772-785` is asserted by nothing.

**Measured on hardware, not read.** Mutant `M3` deletes the disclosure branch, so an operator is
told *"nothing was left behind on this machine"* while un-revokable ACL grants remain:

```
APPLIED_SHA256=8dc05b5c…   MUT_COMPILED=True
M3=dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever ; passed=1 failed=0
M3=live_owner_unreconcilable_lease_is_honoured_not_reclaimed        ; passed=1 failed=0
M3=quarantine_directory_does_not_become_a_second_wedge              ; passed=1 failed=0
M3=reclamation_reports_grants_it_could_not_revoke                   ; passed=1 failed=0
M3=a_leaked_test_lease_is_diagnosed_by_name                         ; passed=1 failed=0
M3_FULLSUITE = 133 passed; 0 failed; 23 ignored
RESTORED_SHA256=bc6bdac1…  RESTORE_MATCHES_PRISTINE=True  REPO_DIRTY_AFTER=0
```

and I re-measured **pristine** myself in the same session: `133 passed; 0 failed; 23 ignored`.
**The mutant is indistinguishable from the fix.**

**Not a blocker, and here is the argument I could not defeat.** The disclosure is a secondary
property the repair *added*, not this finding's subject. Even with zero disclosure the repaired
state strictly dominates refusing forever (§2.4). Critically, the finding's *own* message clause
— *"a message that reads like a platform limitation"* — **is** pinned:
`a_leaked_test_lease_is_diagnosed_by_name` asserts the remedy, the destination, and all three
denied false explanations (`NOT a platform limitation`, `NOT an SSH or session-0 effect`, `NOT
transient`). That is what defeated my own internal pass arguing to keep the row OPEN. Filed
separately; see §6.

### 3.1 My own instrument carried the same defect class, and its guard caught it

First run of `adj-m3.ps1` **aborted itself**: `RESOLVED_COUNT=0;EXPECTED=5`, `WLRC=7`. The
`--list` regex anchored on `$` while every line carries a trailing `CR`, so all five `--exact`
filters resolved to nothing. Without the enumerate-and-assert-the-count guard I would have read
five `ok; 0 passed` runs as five greens and reported the opposite finding. Both halves are kept
in `evidence/28-adj/m3.log`. The instrument that hunts a defect class carries it — instance nine.

I also hit the brief's *first* trap while writing the falsification log: `${PIPESTATUS[0]}`
after a pipeline printed **empty**, so a first version of `gate-falsification.log` recorded
`rc=` for all three variants. Re-run with no pipeline between command and capture.

## 4. Scope statements — are they accurate?

**Yes, both, and the ledger already holds the residual correctly.**

- `KR-05` is **not** closed. `28-H2-SUMMARY` §8 says it measured only the lease surface and did
  not exercise `default_for_platform()` / the `WAYLAND_ALLOW_NO_SANDBOX=1` opt-in. That is a
  faithful restatement of 28-02 §7's own caveat, and the unmeasured half is already carried by a
  **different, still-DEFERRED row**, `F-28-04-002` (MEDIUM, owner Phase 30, `BL-F28-WEDGE-BASHPATH`).
  So a `FIXED` here cannot overstate `KR-05`: the row that would have to move has not moved.
- One nuance worth recording: this repair **narrows but does not moot** `F-28-04-002`. Its
  premise is "a WEDGED lease", and a lease can still wedge through doors this repair did not
  open (§6, `F-28-ADJ-002`).
- `F-28-02-003` / `F-28-02-004` remain MEDIUM/BACKLOG, untouched.

## 5. The gate — and the question of whether moving the row vacates it

**The row moved, the expectation moved with it, and the gate still has teeth. That is checkable,
not asserted.**

`28-04-FINDING-LEDGER.md:1182` said the strict `--validate` *must* fail with exactly one
`F28L-002` on `F-28-02-002`. Measured **before** I changed anything:

```
--self-test                rc=0   (0 failures)
--validate --allow-open    rc=0   63 findings, OK
--validate (strict)        rc=1   REJECTED (1)
    F28L-002  F-28-02-002 (line 39): disposition is OPEN; acceptance requires a terminal disposition
```

Exactly one rejection, on that row and no other. The gate was live and failing.

**Neither broken nor vacuous — because `F28L-002`'s ability to fire never depended on this
production row.** `--self-test` proves it against *synthetic* fixtures — `_ledger_row(
disposition=OPEN)` under both `allow_open` settings, `f28-ledger.py:836` — and does not read
`findings.tsv` at all. A gate whose only proof of life is a permanently-broken production row is
a gate nobody can ever satisfy; that is the opposite of the discipline. So I moved the row **and**
rewrote the expectation to record *which* fork occurred, since the ledger's own wording names the
fork: *"either the finding was repaired or it was laundered, and the difference is the whole
point."* It was repaired.

**Then I proved the tooth is still in, three ways** (`evidence/28-adj/gate-falsification.log`,
every `rc` captured with no pipeline in between):

| Falsification | Result |
|---|---|
| line 39 disposition blanked | `rc=1` — `F28L-002 … no disposition recorded` |
| line 39 set back to `OPEN` | `rc=1` — `F28L-002 … acceptance requires a terminal disposition` |
| line 39 `FIXED` with the executable-check cell emptied | `rc=1` — `F28L-008 … a repair is proved by a check, not asserted` |

The third matters most: **this row could not have been laundered by simply typing `FIXED` into
it.** `F28L-008` fails closed on an unevidenced repair, which is why the row now carries the
executable-check reference.

**Post-update state — all documented checks, real exit codes:**

```
--validate (strict)      rc=0   63 finding(s), allow_open=False   OK
--validate --allow-open  rc=0   OK
--self-test              rc=0   self-test 0 failures; acceptance self-test 0 failures
--check-a2               rc=0   OK      <- accept path still closed on A2 crossings
--check-downgrades       rc=0   OK      <- confirms NO severity downgrade was taken
--check-backlog-ids      rc=0   OK
--check-completeness     rc=0   OK
```

`--check-downgrades` passing is the independent confirmation that this disposition was reached by
repair and not by re-scoring.

## 6. What I am filing rather than absorbing

Absorbing an adjacent defect into a closing row is how a ledger launders itself. Both of these
are **new** findings, neither blocks `F-28-02-002`.

### `F-28-ADJ-001` — MEDIUM — the residual-grant disclosure is guarded by nothing

Measured in §3. `reclamation_report`'s non-empty-`intents` branch has no assertion anywhere; the
test named for it never calls it. A mutant that tells an operator nothing was left behind while
un-revokable ACL grants remain passes 133/133. **What closes it:** make
`reclamation_reports_grants_it_could_not_revoke` call `reclamation_report` and assert the grant
path, the count, and that it does **not** claim nothing was left behind — then re-run `M3` and
require it to fail. Severity MEDIUM: it misinforms an operator, it does not restore the DoS.
→ BACKLOG, non-blocking per the standing severity policy.

### `F-28-ADJ-002` — MEDIUM — the same permanent-wedge shape survives through a different door

**Static reading, NOT reproduced by me — stated as such.** `write_new_synced_lease`
(`storage.rs:138`) is `create_new_nofollow` *then* `write_and_sync`. A crash, kill or job-object
termination between those two leaves a **0-byte `*.toml`** in the lease directory.
`read_validated_lease` rejects it on `metadata.len() == 0` (`storage.rs:257`), and
`recover_dead_leases_locked:651` propagates that with `?` — **aborting the whole recovery pass on
every subsequent `ExecutionIdentity::start`**, which is precisely the `F-28-02-002` shape. The
rewrite path *is* crash-safe (temp + `recover_rewrite_temps`); the initial-create path is not,
and `recover_rewrite_temps` cannot help because `.toml` is not a `.tmp` name.

Distinguish this from `malformed_or_unknown_lease_fails_closed` (`tests.rs:99`), which pins
fail-closed for a *malformed* lease **deliberately** — that one is principled, because a
malformed lease may still represent a live container's grants. A 0-byte file carries no owner and
no grants, so it has exactly as little authority as the case the repair already reclaims. The
distinction the repair draws is sound; this is a gap in it, not a flaw in it.
**What closes it:** plant a 0-byte `.toml` in a test lease root and assert
`recover_dead_leases_locked` survives. → BACKLOG, non-blocking.

## 7. Cross-audit

3/3 `FIXED`, plus an internal pass arguing `OPEN` that did not survive §3's last paragraph.

| Auditor | Position |
|---|---|
| `codex` gpt-5.6-sol | `FIXED` — "the defective disclosure test exposes a real observability/test-coverage defect, but it does not restore or preserve the denial-of-service condition" |
| `gemini` 3.1-pro | `FIXED` — "a distinct quality assurance defect, not a failure to remediate the documented high-severity vulnerability" |
| `kimi` K3 | `FIXED` — "file it as a new finding rather than laundering it through F-28-02-002" |
| internal adversarial | argued `OPEN` on the ground that the finding's text includes the misleading-message clause and the message leg is where the unguarded branch lives. **Rejected on evidence:** the finding's message clause is pinned by `a_leaked_test_lease_is_diagnosed_by_name`; the unguarded branch is *additional* disclosure the repair invented, outside the finding's text. |

Transcripts in `evidence/28-adj/panel-*.txt`. **Codex silently dropped its vote on the first
invocation** — backgrounding it detached stdin and it exited with *"Failed to read prompt from
stdin"*. Re-invoked with stdin attached; the recorded vote is the real one, taken from the **last**
`PANEL_POSITION` match because codex repeats its final block.

## 8. What I did NOT do — read this before treating the gate as green

**The signed certification receipt still says `OPEN`, and I did not touch it.**
`28-04-CERTIFICATION-RECEIPT.json` carries `body.findings[28] = {id: F-28-02-002, disposition:
OPEN}` under an Ed25519 `authority` block over `body_sha256`, which I verified is a sha256 of the
canonicalized body (`separators=(',',':')`, `sort_keys=False`). Editing the disposition
invalidates the hash **and** the signature. **Re-issuing a signed certification receipt is a
release action, not an adjudicator's** — it needs Sean. Until it is re-issued, the ledger and the
receipt disagree, and the receipt is the artifact with a signature on it.

Also left deliberately untouched, and stale in the same way:

- `28-04-PHASE-VERDICT.md` — `:181` `gate_passed=false, unresolved CRITICAL/HIGH = ["F-28-02-002"]`;
  `:236`, `:281`, `:308`, `:343`. It is 28-04's own terminal verdict record. Rewriting another
  plan's verdict from a later lane is the mirror image of the hazard that produced this lane, so
  it is reported, not edited.
- `28-H2-SUMMARY.md` §8's "not merged" line — stale (§1), and it is that lane's record to correct.
- `.planning/REQUIREMENTS.md`, `BACKLOG.md`, the two `HANDOFF-2026-07-28*` files — carry
  `F-28-02-002` as the open blocker.

And: no merge, no PR, no tag, no issue closed, no `wcore-contract generate`. No source file was
changed by this lane — the Windows repo was restored byte-identically (`RESTORE_MATCHES_PRISTINE=
True`, `DIRTY=0`) and recompiled pristine, so the box is left as found.

## 9. Verdict

**`F-28-02-002` — FIXED.** The repair is merged and byte-identical to what was tested; the wedge
is gone on real hardware in both directions; the allow-list introduces no wedge, no writable
surface and no trust crossing; the honour-when-alive leg is mutation-proven and structurally
stronger than claimed; and the one real defect I found is a disclosure gap that cannot restore
the denial of service, filed as `F-28-ADJ-001` rather than absorbed.

The ledger's acceptance gate now passes on the ledger, and it passes while retaining the ability
to fail — proven three ways in §5. **It does not follow that Phase 28 is certified:** the signed
receipt still records `OPEN` and `gate_passed=false`, and only a re-issue by Sean can reconcile
that.

---

_Adjudicated 2026-07-29 · lane `lane/28-adj` · independent of the lane that authored the repair_
