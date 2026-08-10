# A-4 — review someone else's PR

Fixture repo: `gatekeeper`. `main` carries a correct fixed-window limiter; the
branch `pr/sliding-window` replaces it with a sliding-window implementation
carrying **three material defects** and **five plausible distractors** that are
not defects.

The branch's own tests pass. The PR looks green, which is the point — an
under-tested PR is what a review is for.

Grading runs both directions:

* missing any of the three material defects FAILS the row;
* calling any of the five distractors a blocker or a major FAILS the row just as
  hard. A reviewer who flags everything is not a reviewer.

The material defects are substantiated by an executable demonstration
(`keys/a4_pr_review/reference/proof_of_bugs.py`) rather than by opinion, and two
of the distractors are refuted by an executable demonstration
(`proof_distractors.py`). The self-test runs both.

## Build

```
python3 build.py /path/to/workdir/a4
```

Produces a git repo with `main` (tagged `baseline`) and `pr/sliding-window`
checked out.
