# criteria-regrade — working notes (append-only, committed continuously)

Lane: `lane/criteria-regrade`. Worktree:
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-criteria-regrade`.

## Measured SHA

`570056c160a7e497e67bbfe9798aaf3843ac639c` (`fix(lockfile): restore --locked builds, red at
integration head`, 2026-07-30 15:35:02 +0700).

Verified: worktree toplevel is the lane path; `/usr/bin/git status --porcelain` is EMPTY.
(Proxied `git status --short` returned the literal word `ok` — the documented rtk rewrite.
Every number in this lane comes from `/usr/bin/git` redirected to a file and read with the
Read tool, per LANE-BRIEF §3b.)

## Premise check on the orchestrator brief — done FIRST

| Brief claim | Verdict | Evidence |
|---|---|---|
| "grades measured 2026-07-30 at `71acfd19`" (quoting the file header) | **file header is FALSE** | `71acfd19` is an ancestor of HEAD (`merge-base --is-ancestor` = YES) dated 02:28. But `.planning/CRITERIA-STATUS.md` has FOUR commits: `d6a41ecd` 03:08, `25fb1185` 06:03, `e7ef762c` 10:12, `5014f070` 12:04. Rows were re-graded at least three times AFTER the SHA the header names, and the header was never updated. **22 merges landed between `71acfd19` and the file's last edit.** |
| "nine lanes have merged" since | **TRUE**, relative to the file's LAST EDIT | `5014f070..HEAD` contains exactly 9 merge commits (86 commits total), and they are exactly the nine the brief names. Relative to `71acfd19` the count is 31. |

So the brief's list is right but its anchor is wrong: the honest base is `5014f070`, not
`71acfd19`. Both are recorded below and in the rewritten header.

### The nine lanes that landed after the last edit (`5014f070..HEAD`)

```
5265b203 11:31 merge(glibc-reach): lower the Linux glibc floor 2.39 -> 2.34
3595224b 11:32 merge(discord-live): five message actions against a real Discord server
bf95d6a7 11:34 merge(provenance-comparison): per-site provenance findings for the nine notices
c06e1768 11:43 merge(matrix-live): five message actions against matrix.org, and a HIGH
4a3ed957 11:46 merge(slack-live): five legs live, and Slack's exactly-once claim was false too
8f6c80ad 12:31 merge(journey-gate-honesty): the Windows journey gate can now pass
a903142b 14:58 merge(darwin-ci-selfhosted): the task was already done
12b0c18d 15:07 merge(twilio-whatsapp-identity): delivery identity on the wire
c8524ad8 15:34 merge(whatsapp-bridge): opt-in Node bridge, operator-provided
```

## Method

Grade off code and executed tests at HEAD. Never off a `SUMMARY.md`, a lane report, or a
`####` headline. Two rows in the current file are stale *specifically* because someone graded
off a finding lane's summary.

Every row gets a control in BOTH directions (LANE-BRIEF §3b-iii): can this instrument fail,
and can it pass. Absences get a known-positive in the same invocation (§3b-i).

Where I cannot measure: **NOT MEASURED**, and counted. A skip is not a pass.

## Findings log

(appended as measured)
