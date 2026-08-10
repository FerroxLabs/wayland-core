# Hidden answer keys — the whole corpus

**These files are hidden from the agent under test.**  The harness MUST NOT
place `tests/job-corpus/keys/` (or any file under it) inside the workspace the
agent is given, MUST NOT name its contents in the prompt, and MUST NOT let the
agent read this repository path during a run.  A key that leaks is a key that
grades nothing.  If the agent can see this directory, every row here is void
for that run — not degraded, void.

Every key was committed **before** the first execution of its row.  A rubric
written after seeing an output is not a rubric.  They are plain text so that a
human can review whether a key is fair, which a hashed key would prevent.

This file is the union of two rubrics written in parallel lanes.  Section 1
governs rows A-1 .. A-6, section 2 governs rows A-7 .. A-12; both are
authoritative for their own rows and neither supersedes the other.

---

## 1. Rows A-1 .. A-6

**These files are hidden from the agent under test.**  The harness MUST NOT
place `tests/job-corpus/keys/` (or any file under it) inside the workspace the
agent is given, MUST NOT name its contents in the prompt, and MUST NOT let the
agent read this repository path during a run.  A key that leaks is a key that
grades nothing.

Every key was committed **before** the first execution of its row.  A rubric
written after seeing an output is not a rubric.

### Layout

```
keys/<row>/key.json          machine-readable criteria, including FAIL conditions
keys/<row>/README.md         the same criteria in prose, FAIL condition stated first
keys/<row>/hidden_tests/     acceptance tests the harness runs; never shown to the agent
keys/<row>/reference/        a reference solution, used ONLY for the harness self-test
                             (positive control).  Never applied during a graded run.
keys/grade_lib.py            pure-stdlib grading helpers (no product internals)
keys/selftest.py             proves each fixture + key actually discriminates
```

### `key.json` shape

| field | meaning |
|---|---|
| `row` | row id, e.g. `A-2` |
| `fixture` | path to the fixture directory, relative to `tests/job-corpus/` |
| `prompt_file` | the text handed to the agent under test |
| `grades` | ordered list of checks; each has `id`, `kind`, `fail_when` |
| `pass_requires` | ids of checks that must all hold for PASS |
| `notes_only` | ids observed and reported but not scored |
| `run` | how to execute the hidden tests: `cmd`, `cwd`, `pythonpath` (list; join with `os.pathsep`) |

### Rules these keys obey

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

### Host notes the harness must honour

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

### Running the self-test

```
python3 tests/job-corpus/keys/selftest.py            # all rows
python3 tests/job-corpus/keys/selftest.py A-3        # one row
```

The self-test builds every fixture, proves the *unsolved* fixture FAILS its key
(negative control) and the reference solution PASSES it (positive control).  A
key that cannot fail and a key that cannot pass are equally worthless.

---

## 2. Rows A-7 .. A-12

**These are the answers. The agent under test must never be able to read this
directory.**

They are committed, in plain text, in the same commit as the fixture they
grade, and before any run. That is the point: a rubric written after seeing the
output is not a rubric. Plain text also means a human can review whether a key
is fair, which a hashed or encoded key would prevent.

### The one operational rule

The runner copies **`fixtures/<row>/`** into the agent's working directory.
It never copies `keys/`, and the agent's working directory is never inside this
repository.

If the agent can see this directory, every row here is void for that run. Not
degraded — void. There is no partial credit for a test whose answers were on
disk next to it.

### What is in here

| File | Grades | How it decides |
|---|---|---|
| `a07.key.json`, `a07_grade.py` | A-7 | Applies seeded defects and runs the candidate's tests |
| `a07_controls/` | A-7 | A good suite that must PASS, a worthless one that must FAIL |
| `a08.key.json`, `a08_grade.py`, `a08_hidden/` | A-8 | Repository state plus a hidden acceptance suite |
| `a08_selftest.py`, `a08_controls/` | A-8 | Four resolutions, one PASS and three FAIL |
| `a09.key.json`, `a09_probe.py` | A-9 | Starts the service and uses it over HTTP |
| `a09_controls/` | A-9 | A correct service, and one that only forgets durability |
| `a10.key.json` | A-10 | Exact answers for all seven sub-cases |
| `a10_hidden/` | A-10 | The screenshot sub-case's acceptance tests, and the audio onsets |
| `a10_media_grade.py`, `a10_media_selftest.py` | A-10 | text_pdf, scanned_pdf, spreadsheet, audio, video: the exact answers in `a10.key.json`, compared mechanically |
| `a10_degraded_grade.py`, `a10_degraded_selftest.py` | A-10 | Whether the refusal arrived before the guess |
| `a10_tui.key.json` | A-10 attachment | The canary matrix. Drag-and-drop needs a real terminal and is OUT of an unattended run |
| `a10_tui_paste.py` | A-10 attachment | The automatable half: an absolute path pasted into the running product, over all four awkward locations |
| `a11.key.json`, `a11_verify.py`, `a11_selftest.py` | A-11 | Reads the warehouse database directly |
| `a12.key.json` | A-12 | Waypoints, and a prediction against what the change really breaks |
| `a12_grade.py`, `a12_selftest.py` | A-12 | Part 1's eight waypoints in order in real prose; part 2 scored by making the change and running the suite |

### Every key states its own failure condition

Not "criteria for a pass" — the condition under which the row **fails**. A row
whose key cannot describe a failure is not measuring anything, and neither is a
row that can be passed by saying the capability is unavailable. Refusal is a
pass in exactly one place in this whole directory: the degraded-artifact
sub-case, where refusing *is* the correct behaviour, and even there it only
counts when it arrives before any figure.

### Every gate has been shown to fail

A gate that cannot fail is worth as much as one that cannot pass. Each of these
carries a control that was actually run:

```
python3 keys/a07_grade.py --workdir <copy of fixture + keys/a07_controls/good/tests>   # PASS 8/8
python3 keys/a07_grade.py --workdir <copy of fixture + keys/a07_controls/weak/tests>   # FAIL 0/8
python3 keys/a08_selftest.py                                                            # 4/4 agree
python3 keys/a09_probe.py --workdir keys/a09_controls/reference                         # PASS 19/19
python3 keys/a09_probe.py --workdir keys/a09_controls/inmemory                          # FAIL, durability only
python3 keys/a10_degraded_selftest.py                                                   # 7/7 agree
python3 keys/a10_media_selftest.py                                                      # 18/18 agree
python3 keys/a11_selftest.py                                                            # 4/4 agree
python3 keys/a12_selftest.py                                                            # 12/12 agree
```

Two of those deserve a word.

`a09_probe.py` runs the service in a **scratch copy** of `--workdir`. It used to
run it in place, which meant the two commands above wrote `links.db` and
`.a09-service.log` into the committed control directories: a gate self-test that
dirtied the repository it was certifying, and a second run that started from
state the first one left. Verified in both directions on Linux 2026-08-10 —
reference PASS 19/19, near miss FAIL on exactly the two restart checks, and
`git status keys/a09_controls` clean afterwards.

`a12_selftest.py` really applies the `write_through` change and runs the suite
for each of its six part-2 controls, so the scoring is checked against what
breaks, not against a number written down earlier.

Run them again on a new host before trusting a verdict from that host. A green
result from an environment that could not have produced a red one is not
evidence.

### Four states, not three

`PASS`, `FAIL`, `UNPROVEN`, `N/A`. `FAIL` includes "the capability is not
there". `UNPROVEN` means the harness could not get reliable evidence and is
saying so instead of guessing — it is used, for example, when a mutation cannot
be proven to have landed on executable code, or when a media fixture's own
control could not be recovered. `N/A` is for something genuinely out of scope,
and it leaves the denominator.
