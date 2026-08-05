# 23A-C1 — live evidence

All runs on `hetzner-dsm`, worktree `/root/wayland-23a-c1`, branch `hz/23a-c1`.
Commit under test: `5a57670b` unless a section states otherwise.
`fs.inotify.max_user_instances = 512` (already raised; not re-applied by this lane).

---

## 1. The real binary, driven end to end

Not a test harness — the shipped `wcore-skill-govern` executable, with `WAYLAND_HOME`
pinned to a scratch dir so the run is hermetic and touches no real profile.
A **user-authored control skill** (`my-own-skill`) is present throughout: without it, a
binary that revoked everything would satisfy every "it's gone" assertion below.

### 1.1 Before — what the product put in the user's directory

```
governance root: /tmp/23ac1-live-YZ934p/skills-governance

ON DISK (2)
  auto-live-demo  status=present  kind=auto-drafted  path=/tmp/.../skills/auto-live-demo  signature=demo-sig
  my-own-skill  status=present  kind=user-authored  path=/tmp/.../skills/my-own-skill

REVOKED (0)
  (none)
```

### 1.2 Revoke

```
revoked 'auto-live-demo'
  removed from: /tmp/23ac1-live-YZ934p/skills/auto-live-demo
  retained:     2 file(s), 154 byte(s)
  signature:    demo-sig
  revocation id: c8f6893e-02ed-4f17-86b9-1ff6600b3939

This skill will NOT be re-drafted. To undo:
  wcore-skill-govern rollback c8f6893e-02ed-4f17-86b9-1ff6600b3939
```

`ls` of the skills directory afterwards returns exactly one entry — `my-own-skill`.
**The draft is gone from the user's directory; the user's own skill is untouched.**

### 1.3 After — each row reports its OWN status

```
ON DISK (1)
  my-own-skill  status=present  kind=user-authored  path=/tmp/.../skills/my-own-skill

REVOKED (1)
  auto-live-demo  status=revoked  id=c8f6893e-...  revoked_at=2026-07-29T01:39:31...  files=2  bytes=154  restores_to=/tmp/.../skills/auto-live-demo  signature=demo-sig
```

This is the rendering the coordinator's note required: `my-own-skill` carries
`status=present` on its own line while `status=revoked` appears elsewhere in the same
output, so a **bound** matcher reads `present` and an **unbound** one reads `revoked`.
That divergence is asserted in `field_matcher_selftest`.

### 1.4 Rollback, and byte identity proven by hash

```
rolled back e7f44f1f-ca3a-475d-a605-f857fac00393
  restored to: /tmp/23ac1-journey/skills/auto-live-demo
  the drafter is no longer suppressed for this skill

sha_before=2a0568dd478bec01cd9df1992a1a064d56ebd8b2f230073ff1b268abea47504e
sha_after =2a0568dd478bec01cd9df1992a1a064d56ebd8b2f230073ff1b268abea47504e
BYTE_IDENTICAL=yes
```

Compared by `sha256sum`, not by "the file exists" — a restore that re-rendered the body
would pass an existence check and fail this one.

### 1.5 The journal is append-only

`wcore-skill-govern history`:

```
2026-07-29T01:39:51.579065054+00:00  REVOKED         auto-live-demo  (id e7f44f1f-...)
2026-07-29T01:39:51.584296683+00:00  ROLLED-BACK     auto-live-demo  (id e7f44f1f-...)
```

Raw `journal.jsonl` on disk (436 bytes, byte-counted):

```json
{"event":"revoked","revocation_id":"e7f44f1f-...","skill_name":"auto-live-demo","signature":"demo-sig","source_dir":"/tmp/23ac1-journey/skills/auto-live-demo","at":"2026-07-29T01:39:51.579065054+00:00"}
{"event":"rolled_back","revocation_id":"e7f44f1f-...","skill_name":"auto-live-demo","restored_to":"/tmp/23ac1-journey/skills/auto-live-demo","at":"2026-07-29T01:39:51.584296683+00:00"}
```

The rollback **appended**; it did not remove the revocation record.

---

## 2. Red-before-green — the gate can fail

The load-bearing claim is "a revoked draft is not recreated". Proven by reverting exactly
that behaviour and re-running. Mutation applied with `sed` to a `cp`-backed copy; **no git
operation was used**, and the file was restored from the backup (verified: 1 `is_revoked`
site back).

Mutation: `if store.is_revoked(&name, Some(&trigger.signature)) {` → `if false {`
(1 site — count asserted, so a mutation that failed to apply cannot masquerade as a pass).

```
test auto_skill::drafter::tests::revoked_draft_is_not_recreated_and_rollback_restores_it ... FAILED

panicked at crates/wcore-agent/src/auto_skill/drafter.rs:376:14:
a revoked draft must NOT be recreated: DraftResult { name: "auto-code-refactor-review", ... }

test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 2148 filtered out
```

Unmutated, the same target is `7 passed; 0 failed`.

**Discrimination profile.** Under mutation the two positive-path tests
(`drafting_is_unaffected_when_nothing_has_been_revoked`,
`revoking_one_draft_does_not_suppress_a_different_draft`) still PASS. Only the negative-path
test moves. That is the correct profile: the suite is not uniformly sensitive, so a pass is
attributable.

**A trap avoided, recorded because it fired.** The shell reported `MUTATION_EXIT=0` for a
run that had just FAILED — the pipeline into `tail` stole cargo's status, exactly the
LANE-BRIEF §3.2 defect. **The grade above is taken from the `1 failed` count in the
output, not from any exit status.**

---

## 3. Suites (executed counts read back, never exit status alone)

| Target | Result |
|--------|--------|
| `wcore-skills --test govern_revoke_rollback` | `15 passed; 0 failed; 0 ignored; 0 filtered out` |
| `wcore-skills --test govern_cli_drive` | `6 passed; 0 failed; 0 ignored; 0 filtered out` |
| `wcore-skills` (whole crate, isolated) | lib `623 passed; 0 failed; 2 ignored` + every integration target green |
| `wcore-agent --lib auto_skill::` (serial) | `17 passed; 0 failed; 0 ignored; 2138 filtered out` |

The `0 filtered out` on both new suites matters: LANE-BRIEF §3.2 flavour (c) is a filter
matching no test name, which exits 0 having run nothing. Both suites report the exact test
count they contain (15 and 6), and neither has any `#[ignore]`.

The `wcore-agent` run above IS filtered (`2138 filtered out`) — so its `17 passed` is
reported with the filter stated, and the unfiltered serial run is in §4.

## 4. Clippy

`cargo clippy -p wcore-skills --all-targets` → no warnings.
`cargo clippy -p wcore-agent --lib` → no warnings.
(The `imap-proto v0.10.2` future-incompat note is a pre-existing dependency notice, not
this lane's code, and is present at base.)

`cargo fmt --all -- --check` → clean (run on the Mac, the one sanctioned local cargo use).
