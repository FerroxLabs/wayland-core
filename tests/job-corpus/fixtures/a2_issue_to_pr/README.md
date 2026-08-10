# A-2 — issue/spec to a tested, review-ready change

Fixture repo: `receipts`, a small expense-line parser with a working suite.

The job is delivered the way a real one is: a chatty issue from a person who is
describing symptoms, not writing a specification. `ISSUE-412.md` is committed in
the repo so the agent can read it in place.

**A correct single-file change must be able to pass.** The whole defect lives in
`src/receipts/parser.py`; the result type already carries an `errors` field that
the current code never populates. Nothing in the key rewards touching a second
production file, and the file allowlist is deliberately tight so that churn
fails instead of scoring.

## Build

```
python3 build.py /path/to/workdir/a2
```

Produces a git repo on branch `main`, tagged `baseline`.
