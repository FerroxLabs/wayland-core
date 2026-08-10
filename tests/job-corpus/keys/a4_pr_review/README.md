# A-4 key — review someone else's PR

## This row FAILS when any of these is true

1. Any of the three material defects (M1, M2, M3 below) is missing from the
   review, or is present only at `minor`/`nit`/`question` severity, or cites a
   line outside the enclosing function.
2. Any of the five distractors (D1..D5) is reported at `blocker` or `major`
   severity. Inventing a blocker is a failure of review, not an excess of care.
   Mentioning a distractor at `minor`, `nit` or `question` is fine and is
   recorded as a note.
3. A `blocker`-severity finding is raised that is neither a material defect nor
   a listed distractor, and the harness operator cannot produce an executable
   demonstration that it is a real defect. Default disposition for an
   unsubstantiated blocker is FAIL.
4. `review.json` is absent, unparseable, or empty.
5. The agent modified `src/` instead of reviewing it — this row is a review, and
   a rewritten limiter is not a review. (Recorded as a note if the review itself
   is otherwise correct and the source change is additive; FAIL if the review
   is missing.)

## Material defects — substantiated, not asserted

`reference/proof_of_bugs.py` demonstrates all three against the branch code. If
that script ever stops demonstrating them, this row has rotted.

**M1 — the limit is off by one.** `if len(hits) > self.limit` admits
`limit + 1` requests per window. `RateLimiter(limit=2)` lets three through.
Demonstrated.

**M2 — every limiter shares one bucket dict.** `buckets={}` as a default
argument is evaluated once at import, so two `RateLimiter` instances in the same
process share state: a request counted against one limiter is counted against
the other. Demonstrated.

**M3 — `sweep()` raises.** It deletes from `self.buckets` while iterating
`self.buckets.items()`, so the first idle key raises
`RuntimeError: dictionary changed size during iteration`. The README tells
operators to call `sweep()` periodically. Demonstrated.

## Distractors — these are NOT defects

**D1 — `time.monotonic()`.** Tempting to demand a wall clock "so limits survive
a restart" or "so it works across processes". This limiter is documented as
single-process and only ever measures durations; monotonic is the correct
choice and a wall clock would be the bug.

**D2 — `hits.pop(0)` on a list.** O(n) rather than a deque. `n` is bounded by
`limit`, this runs once per request, and the class is documented as a small
per-key window. A performance nit at most.

**D3 — the `<=` asymmetry.** `allow()` prunes on `hits[0] <= cutoff` while
`sweep()` tests `hits[-1] <= cutoff`. It looks like a copy-paste slip. Both are
correct for their purpose: one expires the oldest hit out of the window, the
other asks whether the newest hit is older than a window. Refuted by
`reference/proof_distractors.py`.

**D4 — `setdefault` "mutating a copy".** `hits = self.buckets.setdefault(key, [])`
followed by `hits.pop(0)` / `hits.append(now)` looks like it mutates a local
that never gets stored back. It is the same list object as the one in the dict.
Refuted by `reference/proof_distractors.py`.

**D5 — the `now=None` parameter.** Looks like dead API surface added for no
reason; it is the seam the tests inject time through, and the branch's own tests
use it.

## Severity matching

A material defect counts as found when a finding has `severity` in
`{blocker, major}`, `file` ending `src/gatekeeper/limiter.py`, a `line` inside
the enclosing function's span, and a `message` matching at least one of the
finding's `keywords_any` patterns (case-insensitive regex).
