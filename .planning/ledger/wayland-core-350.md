---
issue: 350
repo: FerroxLabs/wayland-core
title: "[nightly-windows-soak] FAIL - 2026-08-28"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The latency half of the streaming-timeout test refuses to grade when the derived timeout does not dominate the host's own measured timer allowance"
    state: met
    evidence: "symbol:crates/wcore-tools/src/bash/tests.rs::decisive_walk_floor"
    owner: core
    note: "Two changes, both present: timeout_ms is floored at a MEASURED timer allowance (tests.rs:2894-2900), and the latency half gates on floor <= decisive with decisive = (timeout+allowance)*2, the SAME expression the attribution half already used - so the factor was not widened. The allowance is bracketed before AND after the call and the larger is used."
  - id: c2
    text: "On a host that cannot establish the premise, the test skips loudly with the measured numbers instead of panicking"
    state: met
    evidence: "test:crates/wcore-tools/src/bash/tests.rs::the_streaming_bash_timeout_bounds_the_secret_deny_walk"
    owner: core
    note: "On the last attempt it prints SKIP (wayland-core#350) carrying timeout_ms, the measured walk floor, decisive and the measured allowance, then returns. No panic. Earlier attempts grow the tree and re-race."
  - id: c3
    text: "A positive control on a fine-tick host still turns red when the manifest build is moved back outside the timeout scope"
    state: not-met
    owner: core
    note: "Structurally the redness is forced by construction - grading only happens when floor > decisive, floor is the smallest of three fresh-policy walks, and with the manifest build outside the timeout scope the observed value would exceed decisive. But no mutation or positive-control RUN is recorded anywhere in the tree or .planning, and this arm depends on host timer behaviour rather than on a single readable assertion. MUTATION ARM NOT RUN. The structural argument is recorded above, but this criterion asserts an OBSERVED outcome and nothing in the tree records one. The standing rule in this repo is that a test nobody watched fail is not evidence, so it grades not-met until one cheap run flips it."
  - id: c4
    text: "The retries=0 override on this test survives the fix"
    state: met
    evidence: "file:.config/nextest.toml:599"
    owner: core
    note: "the override naming this test by literal operand is correct and should be kept - the failure must not become retryable. A separate script fails if any of the three named tests stops being a literal operand of a retries=0 predicate"
  - id: c5
    text: "The issue's own close condition is met: a green nightly-windows-soak run against this tree"
    state: not-met
    owner: core
    note: "The nightly closes this tracker automatically on a green run and no soak has run against origin/integ/next. Also unclaimed: MASTER-PLAN.md:373 asks that the latency half be confirmed ABSENT FROM THE NEXTEST LIST on Windows rather than self-skipping; as implemented it self-skips, so that check would read as present-and-skipping."
---

This is an auto-filed nightly soak failure. Its body says to grep the build,
nextest and mutants sections, which is boilerplate and is wrong here: the run
log shows a single test failing on both attempts,
the_streaming_bash_timeout_bounds_the_secret_deny_walk in wcore-tools.

It is not stale and it is not the AppContainer ACL race. The failing lines are
byte-identical at the shipped tree - the ship diff to that test file is 77 added
lines, all appended well below them - so this reproduces on v0.13.10.

The root cause is that the instrument is grading below its own noise floor. The
timeout under test is derived as one tenth of a measured walk, which came out at
6 to 7 milliseconds on a host whose timer tick the test itself measured at 15.8
milliseconds. The residue being asserted on is quantization, not product
latency. The other half of the same test already knows this and refuses to grade
when the premise fails; this half does not.

One arm of the fix is verifiable on hetzner, the positive control. The skip arm
needs a coarse-tick host. Criteria come from the cluster C verification note of
2026-08-29.
