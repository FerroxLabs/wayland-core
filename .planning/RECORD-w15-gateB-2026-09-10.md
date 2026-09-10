# RECORD — w15/prog gate-B lane, 2026-09-10

Graded tree: `335328967` (`docs(board): regenerate after the skills activation
gate lands`). Every command below was run on 2026-09-10 from
`worktrees/w15-gateB`. Nothing was pushed. No issue was closed, reopened or
re-milestoned; two comments were posted.

---

## 1. wayland-core#386 — the soak tracker's live run already exists

The issue was filed 2026-08-29 saying the fix "has never executed on GitHub,
and will not until it reaches `main`". It has since reached `main`, and the
run the issue asks for happened on the 2026-09-04 scheduled tick. Nobody went
looking, so the ticket still reads as owing a Sean action it no longer owes.

### 1.1 The red-sibling run

`gh run view 33841172783 -R FerroxLabs/wayland-core` — event `schedule`,
branch `main`, head `509f4426b968a363248beb70d28223ff418e5c62`, run conclusion
`failure`. Job roster as the tracker read it:

    windows-soak                 success
    keyring-blob-size            success
    windows-live-acceptance      failure

That is the shape core#325 c2 asks for: at least one gating job red and at
least one green, in one real run, on the live Octokit.

`gh api repos/FerroxLabs/wayland-core/actions/jobs/100929260910` — the
`Soak tracker (whole-run truth)` job was PRESENT and did not skip:

    3 success Decide from the WHOLE run, not from one job
    4 skipped Close the failure issue on a green RUN
    5 skipped Rehearsal verdict - assert the red sibling did not close anything
    6 success Report red result to issue tracker

Verbatim from step 3's log:

    jobs read      : 3
    failed         : 1
    uninterpretable: 0
    required       : windows-soak keyring-blob-size windows-live-acceptance
    action=report
    reason=job-failed

Step 4 `Close the failure issue on a green RUN :: skipped` is the whole of
#386 c1: a run with a red sibling closed nothing, observed on GitHub rather
than against the stub.

### 1.2 What it posted

Step 6 ran `actions/github-script`, and its log carries the live API call:

    [@octokit/request] "POST https://api.github.com/repos/FerroxLabs/wayland-core/issues" is deprecated.

It opened FerroxLabs/wayland-core#443 at 2026-09-04T06:08:04Z — the same
second as the step — titled `[nightly-windows-soak] FAIL - 2026-09-04`,
labelled `windows-soak` + `test-debt`, whose body reads
`**Failing job(s)**: windows-live-acceptance` and lists the roster. The issue
stayed OPEN for three days and was then auto-closed by the next all-green
tick, which is the behaviour the ticket's own acceptance describes.

### 1.3 Two controls, so the skip is a decision and not a dead path

POSITIVE CONTROL — run 34087589551 (2026-09-07), same workflow, same live
Octokit, roster all `success`: `action=close`, `reason=all-green`, step 4 RAN,
and its log says `closed #443 on a green run`. The close path is reachable, so
its skip on the red run is a verdict rather than a step that never works.

NEGATIVE CONTROL — runs 33947609921 (2026-09-05) and 34014381034 (2026-09-06),
both with `windows-live-acceptance=cancelled`: `action=none`,
`reason=not-conclusive`, and steps 4, 5 AND 6 all skipped. A run the tracker
cannot read whole closes nothing and reports nothing.

### 1.4 The ceiling (#386 c3)

CEILING: a red sibling cannot be produced ON DEMAND by any lane, and the run
above was produced by nature rather than to order. Two separate reasons, and
neither is fixable from a branch:

  * The red came from `windows-live-acceptance`, which runs on the self-hosted
    `ferrox-win-msvc` Windows runner. Nothing in a lane's control makes that
    job fail on request.
  * The on-demand route that DOES exist, the `tracker_rehearsal` dispatch
    input, deliberately replaces both issue-writing steps with a step that
    PRINTS the payload and asserts `action == report`. It exercises the real
    `needs.<job>.result` expansion and the real decision script, and it never
    touches the live Octokit. It is a rehearsal, and the workflow says so.

STATED AS WHAT IT IS: `.github/scripts/tests/soak-tracker-run.test.py` (30
assertions, three red arms, PART C of `soak-tracker-truth.test.sh`, run by
`lint.yml`) drives the real YAML, the real `JOB_RESULTS` interpolation, the
real `soak-tracker-decision.sh` and the real `actions/github-script` bodies
under node against a STUBBED Octokit. It grades the decision arithmetic and
the script bodies. It cannot grade GitHub's scheduler admitting the tracker
job through `if: always()` while a sibling is red — which is the one thing
#386 was opened for, and which §1.1 now supplies.

### 1.5 Limit

The 2026-09-04 run exercised `issues.create`, because no
`[nightly-windows-soak] FAIL` issue was open at the time. The
`issues.createComment` branch — a red run adding to an already-open tracker
issue — has still only ever run against the stub. The issue's own acceptance
bullet is a disjunction ("opened or commented on"), so this is a limit on the
breadth of the live evidence, not a gap in it.

A second limit on §1.1: with no tracker issue open, the close was refused at
the DECISION step, before the issue lookup ran. The positive control in §1.3
is what shows the lookup-and-close half is live.

---

## 2. wayland#1272 c1 — the board gate existed and was wired nowhere

`scripts/check-release-board.py` decides both halves of c1. It fails on a row
that blocks and is missing, a row that no longer blocks, a row whose owner is
empty or `UNOWNED`, and a row whose outstanding count has moved; owners are
READ out of each ledger's `owner:` on its unmet criteria rather than assigned,
so a row cannot claim an owner the ledger does not have.

Run at `335328967`, both arms, without `--write`:

    $ python3 scripts/check-release-board.py --self-test
    SELF-TEST OK: missing, stale, unowned rejected; matching accepted

    $ python3 scripts/check-release-board.py --offline
    OK: board carries all 31 blocking issue(s), each owned, counts current

    $ python3 scripts/check-release-board.py
    OK: board carries all 31 blocking issue(s), each owned, counts current

THE DEFECT FOUND: `grep -rn check-release-board .github/ justfile` returned
NOTHING. The gate was in no workflow, no justfile recipe and no release step.
A drift detector nobody runs satisfies c1's first clause on the day a human
remembers and satisfies its second clause — "the board is updated whenever the
list changes" — never. That is a hand-maintained board wearing a script's
clothes, which is the exact failure the ticket describes.

Closed here by wiring it where the blocking list actually moves: the ci.yml
`ci-linux` step `Release board tracks the blocking list (wayland#1272 c1)`,
plus `just release-board` / `just release-board-live`. Deliberately NOT added
to `just check-all`: the board is the integrator's file, and a gate that reds
on every in-progress lane gets bypassed and then deleted.

### 2.1 The gate then went RED on this lane's own grading, and is left red

Grading `wayland-core#386`'s three criteria `met` in the same commit changed
the blocking list. `python3 scripts/check-release-board.py --offline` now exits
1 with exactly the two rows it should name:

    FAIL: the release board and the blocking list disagree

      ON THE BOARD BUT NO LONGER BLOCKS  FerroxLabs/wayland-core#386
      OUTSTANDING COUNT MOVED  FerroxLabs/wayland#1272: board 3, actual 2

That is a REAL-DRIFT positive control the `--self-test` cannot give: the gate
fired on a list that actually moved, within one commit of it moving.
`.planning/RELEASE-BOARD.md` is generated and is the integrator's file — this
lane is forbidden `--write` — so the board is left stale here deliberately.
**Owed at integration: `python3 scripts/check-release-board.py --write`, before
the new ci-linux step can pass.**

Read literally, c1's second clause is FALSE at this exact tree. It is graded
`met` because the criterion's subject is the board's ability to rot SILENTLY,
and it can no longer do so; on the literal reading c1 could never be met on any
lane branch, which is the same defect c4 on this ticket has. A reader who
disagrees should downgrade it — the evidence for both readings is above.

LIMIT: the CI arm runs `--offline`, because the repo-scoped `GITHUB_TOKEN`
cannot read the second tracker. The online arm additionally corroborates
`kind:` against tracker labels and resolves handoffs, so it can produce a
SMALLER blocking set. Both arms were run by hand above and agreed at 31. If
they ever diverge, the CI step reds on a board that is correct online.

---

## 3. wayland-core#401 c1 — the clean-container arm at the new tree

The skills-activation lane changed this area at `876e5a39a`..`c16983be0`: the
boot prompt no longer carries a per-skill listing at all
(`crates/wcore-agent/src/context.rs::SKILL_DISCOVERY_SECTION`), so the test
now renders the listing through `format_skills_section` on the catalogue boot
actually discovered instead of reading it off `engine.system_prompt()`. The
same lane fixed a VACUOUS precondition: `issue-1150` is also the first two
segments of `UNLISTED_MODEL`, which the prompt's intro states on every
session, so the 80-character arm's "the planted skill reached the listing"
check was passing on the model id. Read off the listing rather than the
prompt, the model id cannot appear, and the honest form is asymmetric — the
roomy arm HOLDS the skill, the tight arm DECLARES it dropped one.

Clean arm re-run at the new tree, slot `parallel-1`:

    WCORE_PROOF_SLOT=parallel-1 python3 tools/remote-proof.py worktrees/w15-gateB \
      nextest run --locked -p wcore-agent --test issue_1150_unknown_context_window_test \
      --retries 0 --no-tests=fail

source `3353289677cbf37b75bb8c6e63a8dddc96b26eb0`, tree
`d552cadc5a951341b07e359dcb7d795e495a0639`, `remote_exit 0`:
`10 tests run: 10 passed, 0 skipped`, including
`an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes`. The
harness exports a per-nonce `WAYLAND_HOME` and `wayland_config_dir` reads it
first, so `user_skills_dir()` resolves into an empty fresh directory — no
ambient non-bundled skills by construction, not merely a clean-looking box.

STILL OWED, and c1 stays NOT-MET for it: the second environment. c1 says
"shown by running it in BOTH", and no run exists of THIS tree on a host whose
`$WAYLAND_HOME/skills` (or `~/.config/wayland-core/skills`) holds a
non-bundled skill. The stabilization harness cannot supply it — it isolates
`WAYLAND_HOME` on every invocation, creates that directory empty, forwards
only a cargo argv and offers no hook to pre-populate it.

---

## 4. wayland-core#404 c1 — still owes a push, not a judgement

Both guard suites re-run on this tree, on the Mac, bash only:

    bash .github/scripts/tests/report-gate-wiring.test.sh   passed: 75  failed: 0
    bash .github/scripts/tests/assert-test-evidence.test.sh passed: 58  failed: 0

The 75/0 includes the ordering arms that pin the checkpoint upload between the
test step and every long gate that follows it, and both controls proving that
comparator can answer NO.

BLOCKED, and precisely: `git show origin/main:.github/workflows/ci.yml | grep
-c nextest-junit-linux-containerized-checkpoint` returns `0`. The checkpoint
step does not exist on `main`; it exists only on this stabilization branch. So
no CI run has ever executed it, and none can until the branch is pushed.
`ci.yml` declares only `pull_request` and `push` on `main` — there is no
`workflow_dispatch` on it, and the file says why (GitHub exposes that trigger
only for workflows already on the default branch). A dispatch cannot
substitute for the push. This lane may not push, so c1 stays not-met.
