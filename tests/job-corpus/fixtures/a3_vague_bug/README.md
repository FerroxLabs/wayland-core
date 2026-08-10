# A-3 — reproduce and fix a real defect from a vague report

Fixture repo: `contacts`, a JSON-file address book with a web-form entry point
and a bulk importer.

The agent gets **the ticket only**. No stack trace, no failing test, no
reproduction, no pointer at a file. Reproducing it *is* the job; handing over a
trace would be the easy version of this row.

## The decisive check

The harness takes the agent's **new** test(s), applies them to the **pre-fix**
revision (tag `pre-fix`, the commit the agent started from) and runs them there.
They MUST fail there, **by assertion**, naming a test the agent added. A
regression test that passes against the broken code proves nothing, and a test
that merely errors out on the old revision proves only that it was written
against a new API.

## Build

```
python3 build.py /path/to/workdir/a3
```

Produces a git repo on branch `main`, tagged `baseline` and `pre-fix` (both on
the same commit; `pre-fix` is the name the key uses).
