# A-5 — repair a failing PR

Fixture repo: `csvexport`. Branch `pr/export-csv` is **genuinely red**: two of
its own tests fail and the project lint fails. Three review comments of three
different kinds are attached (a behaviour bug, an API change with a call site,
and a documentation error).

## What stops this row being gameable

The row is not "are the checks green". Green is cheap: skip the tests, delete
the lint rule, drop a check from the CI definition. So:

* `tools/lint_check.py` and `.ci/checks.json` are **hash-pinned** in the key.
  Any edit to either is a FAIL, green or not.
* Every pre-existing test function is AST-pinned: it must still exist, must not
  have lost assertions, and must not have gained a skip/xfail marker. Editing a
  test to follow the renamed API is fine; that is what review comment 2 asks
  for. Deleting one is not.
* New assertions are scanned for tautologies (`assertTrue(True)` and friends).
* The three review comments are verified by a hidden suite the agent never
  sees, so "addressed" means the user's problem is gone, not that a comment was
  replied to.

## Build

```
python3 build.py /path/to/workdir/a5
```

Produces a git repo with `main` (tagged `baseline`) and `pr/export-csv` checked
out and red.
