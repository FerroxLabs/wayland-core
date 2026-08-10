# Job-corpus fixtures — A-1 .. A-6

Six self-contained fixture repositories. Each is stored as a plain `tree/`
directory plus a `build.py` that materialises it into a **real git repository**
with real history, deterministic authorship and fixed commit dates. Storing them
as trees rather than nested `.git` directories keeps them reviewable here and
keeps the built repo byte-identical on Linux, macOS and Windows.

```
python3 tests/job-corpus/fixtures/<row>/build.py /path/to/workdir/<row>
```

Prerequisites: `git` and Python 3 (stdlib only). Nothing else, on any platform.

| row | fixture | the job | what the agent is given |
|---|---|---|---|
| A-1 | `a1_cold_start` | add the missing conversion to a tiny package | `PROMPT.md` |
| A-2 | `a2_issue_to_pr` | pick up a chatty issue and open a PR | `PROMPT.md` + `ISSUE-412.md` in the repo |
| A-3 | `a3_vague_bug` | "saving occasionally duplicates records" | `PROMPT.md` + `TICKET-8871.md` in the repo — no trace, no repro |
| A-4 | `a4_pr_review` | review a green-looking PR | `PROMPT.md`, branch `pr/sliding-window` |
| A-5 | `a5_red_pr` | repair a red PR with three review comments | `PROMPT.md`, branch `pr/export-csv` |
| A-6 | `a6_migration` | upgrade a vendored dependency across the repo | `PROMPT.md`, `vendor/tokenlib-2.0.0/CHANGELOG.md` |

The hidden acceptance criteria for all six live in `../keys/`. **The harness must
never expose `../keys/` to the agent under test.** Each row's key states, in
prose and in `key.json`, the exact condition under which the row FAILS.

## Branch layout per fixture

| fixture | branches / tags |
|---|---|
| A-1 | `main`, tag `baseline` |
| A-2 | `main`, tag `baseline` |
| A-3 | `main`, tags `baseline` and `pre-fix` (same commit; `pre-fix` is the name the key uses) |
| A-4 | `main` (tag `baseline`) + `pr/sliding-window` checked out |
| A-5 | `main` (tag `baseline`) + `pr/export-csv` checked out, red |
| A-6 | `main`, tag `baseline` |

## Proving the fixtures still discriminate

```
python3 tests/job-corpus/keys/selftest.py
```

Builds every fixture and asserts, for each row, that the untouched fixture FAILS
its key and the reference solution PASSES it — plus, for A-3, A-5 and A-6, that
a plausible wrong answer is caught. Run it before trusting a result from any
row.
