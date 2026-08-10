# Answer keys — A-7 through A-12

**These are the answers. The agent under test must never be able to read this
directory.**

They are committed, in plain text, in the same commit as the fixture they
grade, and before any run. That is the point: a rubric written after seeing the
output is not a rubric. Plain text also means a human can review whether a key
is fair, which a hashed or encoded key would prevent.

## The one operational rule

The runner copies **`fixtures/<row>/`** into the agent's working directory.
It never copies `keys/`, and the agent's working directory is never inside this
repository.

If the agent can see this directory, every row here is void for that run. Not
degraded — void. There is no partial credit for a test whose answers were on
disk next to it.

## What is in here

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
| `a10_degraded_grade.py`, `a10_degraded_selftest.py` | A-10 | Whether the refusal arrived before the guess |
| `a10_tui.key.json` | A-10 attachment | The canary matrix, graded on a real terminal |
| `a11.key.json`, `a11_verify.py`, `a11_selftest.py` | A-11 | Reads the warehouse database directly |
| `a12.key.json` | A-12 | Waypoints, and a prediction against a recorded truth |

## Every key states its own failure condition

Not "criteria for a pass" — the condition under which the row **fails**. A row
whose key cannot describe a failure is not measuring anything, and neither is a
row that can be passed by saying the capability is unavailable. Refusal is a
pass in exactly one place in this whole directory: the degraded-artifact
sub-case, where refusing *is* the correct behaviour, and even there it only
counts when it arrives before any figure.

## Every gate has been shown to fail

A gate that cannot fail is worth as much as one that cannot pass. Each of these
carries a control that was actually run:

```
python3 keys/a07_grade.py --workdir <copy of fixture + keys/a07_controls/good/tests>   # PASS 8/8
python3 keys/a07_grade.py --workdir <copy of fixture + keys/a07_controls/weak/tests>   # FAIL 0/8
python3 keys/a08_selftest.py                                                            # 4/4 agree
python3 keys/a09_probe.py --workdir keys/a09_controls/reference                         # PASS 19/19
python3 keys/a09_probe.py --workdir keys/a09_controls/inmemory                          # FAIL, durability only
python3 keys/a10_degraded_selftest.py                                                   # 7/7 agree
python3 keys/a11_selftest.py                                                            # 4/4 agree
```

Run them again on a new host before trusting a verdict from that host. A green
result from an environment that could not have produced a red one is not
evidence.

## Four states, not three

`PASS`, `FAIL`, `UNPROVEN`, `N/A`. `FAIL` includes "the capability is not
there". `UNPROVEN` means the harness could not get reliable evidence and is
saying so instead of guessing — it is used, for example, when a mutation cannot
be proven to have landed on executable code, or when a media fixture's own
control could not be recovered. `N/A` is for something genuinely out of scope,
and it leaves the denominator.
