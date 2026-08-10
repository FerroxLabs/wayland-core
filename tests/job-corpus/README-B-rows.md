# Job corpus — Tier B fixtures and keys (B-1 … B-5)

Operator reach. Five rows, six fixtures, five graders. Everything here is
standard-library Python 3 and drives the product as a black box: no fixture and
no grader imports, links to, or reads Wayland Core source, and none of them
grades anything the product says about itself.

| row | fixture | grader | what the user gets |
|---|---|---|---|
| B-1 (blocker) | `fixtures/b1-durable-job/` | `graders/grade_b1.py` | tonight's shipments booked exactly once across a kill and a resume |
| B-2 | `fixtures/b2-provider-failure/` | `graders/grade_b2.py` | the month-end report, correct, after the provider died mid-job |
| B-3 | `fixtures/b3-oncall-approval/` | `graders/grade_b3.py` | an approved dependency migration landed at 3am with a sleeping human's answer |
| B-4 | `fixtures/b4-remote-build/` | `graders/grade_b4.py` | a release tarball built on the build host, and that host left usable after a cancel |
| B-5a | `fixtures/b5a-browser/` | `graders/grade_b5.py --fixture browser` | the stock order actually placed in the depot tool |
| B-5b | `fixtures/b5b-native-app/` | `graders/grade_b5.py --fixture native` | the licence actually activated on the desktop app |

Hidden answer keys: `keys/b-1.key.md` … `keys/b-5.key.md` (plus machine-readable
`.key.json`, and hidden acceptance tests under `keys/b-2/`, `keys/b-3/`). Every
key states its FAIL conditions explicitly, and every key says what makes the row
UNPROVEN rather than FAIL. **Refusal is never a pass in any of these rows.**

## Run the graders' own controls first

A grader that cannot fail is worth nothing, so each one ships with synthetic
worlds containing the exact defects it exists to catch:

```
python3 graders/grade_b1.py --self-test      # 9/9
python3 graders/grade_b2.py --self-test      # 9/9
python3 graders/grade_b3.py --self-test      # 11/11
python3 graders/grade_b4.py --self-test      # 13/13
python3 graders/grade_b5.py --self-test      # 16/16
python3 fixtures/b3-oncall-approval/mail_smoke.py   # 11/11, the mail host itself
```

If any of these is short, fix the harness before reading a single product result.

## Evidence discipline

Each fixture README states an evidence contract: which files, and **which
process wrote each one**. Only files written by a harness-owned observer — the
shipping service, the fault proxy, the mail host, the build host's collector,
the depot server, the licence application — decide PASS. `run.json` records what
the harness did; it never carries a verdict.

Missing evidence is UNPROVEN, never PASS and never FAIL.

## Where these rows can and cannot be run

* B-1, B-2, B-3, B-5a: any of the three platforms, hermetic, no credentials.
  B-2 needs a real provider upstream to relay to.
* B-4: needs a **second machine**. Without one the row is UNPROVEN. Running the
  "remote" build locally defeats the build-seed check and proves nothing.
* B-5b: needs a display. On Linux that means Xvfb and `python3-tk`. A claimed
  platform with no display is a FAIL, recorded honestly — not N/A.
