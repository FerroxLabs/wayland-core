# Phase 20 — Plan Reconciliation

Read-only reconciliation of every `*-PLAN.md` in
`.planning/phases/20-transactional-delegated-mutation/`.
Branch: `plan/f20-unified-audit-repair` (repo `/Users/seandonahoe/dev/waylandcore-ferrox`).
Generated: 2026-07-25. No source, plan, SUMMARY, ROADMAP, PROJECT, or state file was modified.

## The FOUR Phase 20 Success Criteria (sole authority for column 4)

1. Parallel children can make conflicting edits without overwriting or silently mutating the parent.
2. Stale identity, failed gates, and conflicts stop before merge while preserving usable evidence.
3. Snapshot, workspace, journal, receipt, merge, rollback, cleanup, and native Windows/macOS identities share one authoritative lifecycle.
4. One exact F20 successor lands on the admitted candidate with focused and aggregate proof.

**Column 4 rule applied:** a plan is `REQUIRED (SC#n)` only if it traces to one of the four
criteria above. Plan self-description, stated severity, and blocking prose were ignored.
Plans whose only tie to Phase 20 is `REQ-native-r*` (the additive native-UAT repair
requirements r1–r15, being split out into a separate phase) are `NOT-REQUIRED` and tagged
`[native-split]` so they can be moved rather than archived.

**Column 3 note:** SHAs come from `/usr/bin/git log --all` on this repo. Where a plan's work
landed on a separate worktree branch and only a docs/SUMMARY commit exists on this branch,
both are shown — the `source_sha` recorded in that plan's SUMMARY frontmatter is given as
`src <sha>` and was verified to exist as a commit object in this repo. `(plan only)` marks
plans for which the only commit is the plan-authoring commit.

## Table

| plan | has SUMMARY? | source commit if landed | REQUIRED-FOR-CRITERIA? |
|---|---|---|---|
| 20-01 | YES | `9f0c8e09` (merge of `worktree-agent-f20-01`; impl `df987391`…`d12d6a0c`) | REQUIRED (SC3) |
| 20-02 | YES | `c09b4d62` (merge), `4dcd62ae`, `2ebf46a2`, `ba4f1a2a`, `fd241f13`, `96afb30a`, `a1187943` | REQUIRED (SC3) |
| 20-03 | YES | `f7249b64` (merge), `d343fc72` (final source), `8738b24e`…`bcd8463a` | REQUIRED (SC1) |
| 20-04 | YES | `2afecb3a`, `0e51a12c`, `d5b9f9a0`, `98bdc961`, `30a2b4f1`, `c0ac6721` | REQUIRED (SC1) |
| 20-05 | YES | `5a30ea88` (summary commit only; src `a528dbc7`) | REQUIRED (SC1) |
| 20-06 | YES | `b188ab47` (summary commit only; src `10d75737`) | REQUIRED (SC2) |
| 20-07 | YES | `21329d08` (summary commit only; src `d527ca87`) | REQUIRED (SC2) |
| 20-08 | YES | `5e665ec5`, `f9c33a94`; repaired successor src `6937ef61`, re-pointed by `af645ace` | REQUIRED (SC3) |
| 20-09 | YES | `8d662771`, `1ace6960` | REQUIRED (SC2) |
| 20-10 | YES | `542b917e` (summary commit only; src `b1de8903`) | REQUIRED (SC2) |
| 20-11 | YES | `5cd67c5a`, `bb97519e` | REQUIRED (SC2) |
| 20-12 | YES | `e19cf4ed` (summary commit only; src `b8a260ec`) | REQUIRED (SC2) |
| 20-13 | YES | `9d67fdd9` (summary commit only; src `ace4bd26`) | REQUIRED (SC2) |
| 20-14 | YES | `bc907b73`, `8321f8e2` | REQUIRED (SC2) |
| 20-15 | YES | `c2e4d594`, `2e26698f` | REQUIRED (SC1) |
| 20-16 | YES | `33c7938d`, `52d0495d`, `8709b955`, `bc0f6d52`, `e3e3489a` | REQUIRED (SC3) |
| 20-17 | YES | `204e167e`, `fb5ce8f2`, `f2d92e05` | REQUIRED (SC4) |
| 20-18 | YES | `c9b31e42` (plan), `82c51cce`, `1e778d9b` | REQUIRED (SC4) |
| 20-19 | YES | `3e48ad65` (plan), `fadf15e1` | NOT-REQUIRED `[native-split]` |
| 20-20 | YES | `629c810e`, `31844fb1`, `772b2b93` | NOT-REQUIRED `[native-split]` |
| 20-21 | YES | `b533c48c`, `9cf5666e`, `c2b26c02` | NOT-REQUIRED `[native-split]` |
| 20-22 | YES | `d49d0ba7` | NOT-REQUIRED `[native-split]` |
| 20-23 | YES | `95c81ec6`, `43c42916` | NOT-REQUIRED `[native-split]` |
| 20-24 | YES | `25c3c484`, `72ec80f2` | NOT-REQUIRED `[native-split]` |
| 20-25 | YES | `e9f9e95e`, `03949633` | NOT-REQUIRED `[native-split]` |
| 20-26 | NO | `3e48ad65` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-27 | NO | `3e48ad65` (plan only), `03949633` (plan amendment) | NOT-REQUIRED `[native-split]` |
| 20-28 | NO | `3e48ad65` (plan only) | REQUIRED (SC4) |
| 20-29 | YES | `ea139f4b` (plan), `346c4ead` | NOT-REQUIRED `[native-split]` |
| 20-30 | YES | `978478cb` | NOT-REQUIRED `[native-split]` |
| 20-31 | YES | `47f5d61b`, `983df765` | NOT-REQUIRED `[native-split]` |
| 20-32 | YES | `787af5ac` | NOT-REQUIRED `[native-split]` |
| 20-33 | NO | `ea139f4b` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-34 | NO | `ea139f4b` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-35 | NO | `ea139f4b` (plan only) | REQUIRED (SC4) |
| 20-36 | YES | `a9f94615` (plan), `8b8cf5bf`, `daf27337`, `fd486d05` | NOT-REQUIRED `[native-split]` |
| 20-37 | YES | `a4a6116b` | NOT-REQUIRED `[native-split]` |
| 20-38 | NO | `a9f94615` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-39 | NO | `a9f94615` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-40 | NO | `a9f94615` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-41 | NO | `a9f94615` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-42 | NO | `a9f94615` (plan only) | REQUIRED (SC4) |
| 20-43 | YES | `7cbc1d88` (plan), `76da033f`, `c9258219`, `230e4521`, `92cac8bb`, `5f433dd1` | NOT-REQUIRED `[native-split]` |
| 20-44 | YES | `3f839309`, `d45efe4f` | NOT-REQUIRED `[native-split]` |
| 20-45 | YES | `f1dcf73b` (4-way gate revision), `8aeb7c6a` | NOT-REQUIRED `[native-split]` |
| 20-46 | NO | `7cbc1d88` (plan only), `4ca0a8f7` (plan amendment) | NOT-REQUIRED `[native-split]` |
| 20-47 | NO | `7cbc1d88` (plan only), `f1dcf73b` (plan amendment) | NOT-REQUIRED `[native-split]` |
| 20-48 | NO | `7cbc1d88` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-49 | NO | `7cbc1d88` (plan only) | REQUIRED (SC4) |
| 20-50 | YES | `db16d18c` (plan), `472d7c03`, `f0dd5b6d`, `b8a1c9d4` | NOT-REQUIRED `[native-split]` |
| 20-51 | YES | `eff0b3be` | NOT-REQUIRED `[native-split]` |
| 20-52 | YES | `8593d2b5` | NOT-REQUIRED `[native-split]` |
| 20-53 | NO | `db16d18c` (plan only), rebound by `c902b2e5` then `d1c168bc` | NOT-REQUIRED `[native-split]` |
| 20-54 | NO | `db16d18c` (plan only), rebound by `c902b2e5` then `d1c168bc` | NOT-REQUIRED `[native-split]` |
| 20-55 | NO | `db16d18c` (plan only), rebound by `c902b2e5` then `d1c168bc` | NOT-REQUIRED `[native-split]` |
| 20-56 | NO | `db16d18c` (plan only), rebound by `c902b2e5` then `d1c168bc` | REQUIRED (SC4) |
| 20-57 | YES | `2102e224` (plan), `8a1d2d84`, `fe4432cf` | NOT-REQUIRED `[native-split]` |
| 20-58 | YES | `c902b2e5` (plan), `7b2462a6` | NOT-REQUIRED `[native-split]` |
| 20-59 | YES | `c902b2e5` (plan), `e7254603` | NOT-REQUIRED `[native-split]` |
| 20-60 | NO | `d1c168bc` (plan), `fe7dd8ea` | NOT-REQUIRED `[native-split]` |
| 20-61 | NO | `d1c168bc` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-62 | NO | `d1c168bc` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-63 | NO | `56e47328` (plan), `e0fa300a`, `2214c38c`, `00f453f2` | NOT-REQUIRED `[native-split]` |
| 20-64 | NO | `56e47328` (plan), `84416b9a`, `aaeeb379` | NOT-REQUIRED `[native-split]` |
| 20-65 | NO | `56e47328` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-66 | NO | `56e47328` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-67 | NO | `56e47328` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-68 | NO | `56e47328` (plan only) | NOT-REQUIRED `[native-split]` |
| 20-69 | NO | `a87c0ad2` (plan), `0e867008` | NOT-REQUIRED `[native-split]` |
| 20-71 | NO | `ce148c3a` (plan), `919629ee` | NOT-REQUIRED `[native-split]` |
| 20-72 | NO | — (PLAN.md is untracked; no commit references 20-72) | NOT-REQUIRED `[native-split]` |
| 20-73 | YES | `a1af7707` (plan+summary commit; src `646ecda8`) | NOT-REQUIRED `[native-split]` |
| 20-74 | NO | — (PLAN.md is untracked; no commit references 20-74) | NOT-REQUIRED `[native-split]` |
| 20-75 | YES | `ce9a11a6` (plan+summary commit; src `c39f7254`), `dbc60a82` (temp probe, to be reverted) | NOT-REQUIRED `[native-split]` |

## Counts

- Total plan files: **74** (`20-01` … `20-75`; `20-70` does not exist; no letter-suffixed plans exist)
- Have a matching `*-SUMMARY.md`: **42**
- Do not have a SUMMARY: **32**
- `REQUIRED`: **23** — SC1 ×4 (20-03, 20-04, 20-05, 20-15); SC2 ×8 (20-06, 20-07, 20-09, 20-10, 20-11, 20-12, 20-13, 20-14); SC3 ×4 (20-01, 20-02, 20-08, 20-16); SC4 ×7 (20-17, 20-18, 20-28, 20-35, 20-42, 20-49, 20-56)
- `NOT-REQUIRED`: **51**
- Tagged `[native-split]`: **51** (every NOT-REQUIRED plan)

## Superseded generations

Repeating fix → re-seal → cross-audit → native-dispatch → post-native-review → re-prep →
authorize cycle. Later generations re-plan the same steps against a new candidate SHA.

- **Fix/repair step:** 20-19..20-22 → 20-29 → 20-36 → 20-43 → 20-50 → 20-57 → 20-60 → 20-63/64/65 → 20-71/72/73/74 → **20-75 (live)**
- **Re-seal step:** 20-23 → 20-30 → 20-37 → 20-44 → 20-51 → 20-58 → **20-61 (live)**
- **Pre-native cross-audit step:** 20-24 → 20-31 → 20-38 → 20-45 (upgraded to 4-way panel by `f1dcf73b`) → 20-52 → 20-59 → 20-62 → **20-66 (live)**
- **Native-proof dispatch step:** 20-25 → 20-32 → 20-39 → 20-46 → **20-53 (live, rebound onto the 20-61 seal)**; 20-68 is the 20-63..67 generation's dispatch
- **Post-native review step:** 20-26 → 20-33 → 20-40 → 20-47 → **20-54 (live)**
- **Re-prep / reauthentication step:** 20-27 → 20-34 → 20-41 → 20-48 → **20-55 (live)**
- **F20 aggregate-authorization step (the SC4 chain):** 20-18 → 20-28 → 20-35 → 20-42 → 20-49 → **20-56 (live)**
- **20-46..20-49** were explicitly superseded by `db16d18c` ("supersede RED-3f839309-bound 20-46..49")
- **20-08 candidate:** original source `5e665ec5` superseded by repaired successor `6937ef61` (re-pointed by `af645ace`, re-reviewed by 20-16)
- **20-03:** a `20-03R-PLAN.md` (swarm isolation repair, `dfadd656`) existed and was folded back into 20-03 (`13974b0e`); the live plan is 20-03
- **20-74 / 20-75:** both frontmatters open with the identical "PHASE-20 NATIVE WINDOWS CLOSEOUT" truth; 20-75 `depends_on: 20-74`, so 20-75 is the live one

## Ambiguous, defaulted to NOT-REQUIRED

These brush SC3's clause "…and native Windows/macOS identities share one authoritative
lifecycle", but their frontmatter requirement IDs are exclusively `REQ-native-r*`, so the
native-split rule was applied:

- 20-19 (native sandbox reset/identity repair, r1/r2/r3/r15)
- 20-20 (AppContainer isolation identity proofs, r2/r5/r6)
- 20-43 (Windows `\\?\` canonicalize + cmd quoting production root causes, r2..r7)
- 20-65 (Class B `fs_read_deny` — described as a real production security defect, r2/r4)
- 20-71 (wcore-swarm path representation centralization incl. BL-5 fail-open, r6/r4/r12)
- 20-73, 20-75 (native Windows closeout: directory authority / LockFileEx, r4/r6/r12)

## Observations (not acted on)

- `20-72-PLAN.md` and `20-74-PLAN.md` are untracked in git; every other PLAN file is committed.
- `20-70-PLAN.md` does not exist, yet commit `334f264d` is subject-tagged `test(20-70)`.
- 32 of 74 plans have no SUMMARY; 21 of those have only a plan-authoring commit.
- 20-05, 20-06, 20-07, 20-10, 20-12, 20-13 landed on branches whose source SHAs appear only in SUMMARY frontmatter, not in any commit subject on this branch.
- Commit `dbc60a82` is self-labelled "TEMP PROBE 20-75 … (to be reverted)" and is still present in history.
