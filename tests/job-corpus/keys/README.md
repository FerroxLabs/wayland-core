# Hidden answer keys — A-1 .. A-6

**These files are hidden from the agent under test.**  The harness MUST NOT
place `tests/job-corpus/keys/` (or any file under it) inside the workspace the
agent is given, MUST NOT name its contents in the prompt, and MUST NOT let the
agent read this repository path during a run.  A key that leaks is a key that
grades nothing.

Every key was committed **before** the first execution of its row.  A rubric
written after seeing an output is not a rubric.

## Layout

```
keys/<row>/key.json          machine-readable criteria, including FAIL conditions
keys/<row>/README.md         the same criteria in prose, FAIL condition stated first
keys/<row>/hidden_tests/     acceptance tests the harness runs; never shown to the agent
keys/<row>/reference/        a reference solution, used ONLY for the harness self-test
                             (positive control).  Never applied during a graded run.
keys/grade_lib.py            pure-stdlib grading helpers (no product internals)
keys/selftest.py             proves each fixture + key actually discriminates
```

## `key.json` shape

| field | meaning |
|---|---|
| `row` | row id, e.g. `A-2` |
| `fixture` | path to the fixture directory, relative to `tests/job-corpus/` |
| `prompt_file` | the text handed to the agent under test |
| `grades` | ordered list of checks; each has `id`, `kind`, `fail_when` |
| `pass_requires` | ids of checks that must all hold for PASS |
| `notes_only` | ids observed and reported but not scored |
| `run` | how to execute the hidden tests: `cmd`, `cwd`, `pythonpath` (list; join with `os.pathsep`) |

## Rules these keys obey

1. No pass condition names a product-internal noun.  Every condition is stated
   as something the *user* got.
2. No gate is unfailable: each check states the condition under which it FAILS.
   "The agent said it could not do it" is never a PASS.
3. Five states: PASS / FAIL / UNPROVEN / N/A / note.  A key may only emit
   UNPROVEN for an infrastructure failure of the harness itself (fixture would
   not build, git unavailable), never for a weak agent result.
4. The agent's own transcript, receipts, summary or self-reported test output
   are **never** evidence.  Every check reads the filesystem, git, or the output
   of a test run the harness started itself.

## Host notes the harness must honour

* Every `cmd` in a `key.json` is written with `python3`. On Windows substitute
  the interpreter the harness is running under (`sys.executable`); nothing else
  in the command changes.
* `pythonpath` is a **list**; join it with `os.pathsep`, never with `:`.
* `<repo>` is the agent's worktree, `<keys_dir>` is this directory. Placeholders
  are substituted by the harness, never by the fixture.
* Building a fixture needs `git` on `PATH` and nothing else. Fixtures carry a
  `.gitattributes` of `* -text` so a Windows checkout is byte-identical, and
  every hash in a key is taken over newline-normalised content.
* Each fixture's `tests/__init__.py` exists only so that
  `unittest discover -s tests -t .` works on Python 3.11+, where namespace
  packages are no longer discoverable.

## Running the self-test

```
python3 tests/job-corpus/keys/selftest.py            # all rows
python3 tests/job-corpus/keys/selftest.py A-3        # one row
```

The self-test builds every fixture, proves the *unsolved* fixture FAILS its key
(negative control) and the reference solution PASSES it (positive control).  A
key that cannot fail and a key that cannot pass are equally worthless.
