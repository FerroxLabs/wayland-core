---
issue: 404
repo: FerroxLabs/wayland-core
kind: defect
title: "A ci-linux cancellation at the 120-minute budget destroys the JUnit evidence the run already produced"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "JUnit evidence produced by the test step survives a cancellation of a LATER step in the same job, demonstrated on a real run rather than argued."
    state: not-met
    owner: core
    note: "TEXT RESTORED 2026-09-03. All three criteria in this file were truncated on transcription: each began at the colon after its bold `c1`/`c2`/`c3` marker and stopped at the first newline of the issue's wrapped bullet, so c1 read ': JUnit evidence produced by the test step survives a cancellation of a' -- a fragment that cannot be graded either way. A criterion nobody can read is a criterion nobody can fail. Restored verbatim from the issue body (Acceptance section, lines 48-54). The claim itself is UNCHANGED and still not-met: nothing in the tree uploads JUnit before the steps that can consume the wall, so a cancellation still takes the evidence with it."
  - id: c2
    text: "A `ci-linux` job that reaches its budget is distinguishable in the `report` job's own output from one that produced no evidence at all - the two are currently the same observable."
    state: not-met
    owner: core
    note: "TEXT RESTORED 2026-09-03, same transcription defect as c1. Still not-met, and this session is direct evidence for why it matters: `report` failed 13 consecutive runs and every diagnosis had to be made by downloading each leg's junit artifact by hand, because the job's own output cannot say whether a missing leg was killed at the wall or never produced anything. `.github/scripts/assert-test-evidence.sh` already carries the HINT text for the second case ('A leg that dies before its test step ... leaves no artifact'), which is exactly the sentence that makes the two indistinguishable when the first case happens."
  - id: c3
    text: "The measured wall time of `ci-linux` against its budget is recorded, so the next raise is a decision rather than a reaction."
    state: met
    evidence: "file:.github/workflows/ci.yml:1255:Median 102.0 min"
    owner: core
    note: "MET 2026-09-03 by measurement recorded IN THE TREE, at the timeout it governs, next to the table it extends. n=9 consecutive runs of this job, 2026-09-02/03: 87.0, 95.5, 97.0, 100.1, 102.0, 105.5, 116.1, 122.8, 123.3 min. Median 102.0 (68% of the 150 limit), max 123.3 (82%), min 87.0 (58%). CORRECTION MADE WHILE MEASURING, and it is the substance rather than a detail: the first pass graded these against 120 minutes because `timeout-minutes: 120` appears earlier in ci.yml -- that value belongs to the matrix job (`CI (Array)`/macOS/Windows), and ci-linux's own is 150 at ci.yml:1258. Grading a budget against the wrong budget is how a 82% reading becomes a 103% panic. THE FINDING IS THAT THE MEDIAN IS THE WRONG STATISTIC: the block's own '~50% over the measured figure' is true of the median and false of the maximum, which sits 18% under the wall -- the same 18% that block calls 'too thin for a job of this length' when it justified moving 90 to 150. The spread is 87.0-123.3, a 36-minute band. NOT RAISED, deliberately: no run in the sample was killed and the median has not moved (99-102 before, 102.0 now), and the block is explicit that widening the gap costs the timeout its ability to catch a hang. The trigger for the next raise is recorded with the table so it is a decision and not a reaction -- one KILLED run, or a median above 115 min (77%), whichever comes first."
---

# Three criteria nobody could read

This ledger's real defect was its own transcription: every criterion was cut at
the first line-wrap of the issue's bullet, leaving fragments that begin with a
colon. A criterion that cannot be read cannot be graded, and cannot be failed --
which is the same failure mode as a gate with no reachable pass state, arrived at
from the other side.

Texts are restored verbatim from the issue. c3 is now met with a measurement
recorded at the timeout it governs; c1 and c2 are unchanged and still owe code.
