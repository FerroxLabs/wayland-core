---
issue: 350
repo: FerroxLabs/wayland-core
title: "[nightly-windows-soak] FAIL - 2026-08-28"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The latency half of the streaming-timeout test refuses to grade when the derived timeout does not dominate the host's own measured timer allowance"
    state: not-met
    owner: core
    note: "the timeout is derived as walk/10, which was 6-7ms on the failing host, while the same test measured that host's one-timer-wait cost at 15.8ms. The quantity under test is 2.6x smaller than the instrument's own resolution, and only one allowance is subtracted while a reschedule pays several ticks"
  - id: c2
    text: "On a host that cannot establish the premise, the test skips loudly with the measured numbers instead of panicking"
    state: not-met
    owner: core
    note: "the attribution half of this same instrument already refuses to grade on Windows when the floor is within twice the timeout plus allowance, citing a red that reported a busy host as a product defect. The latency half was left grading on every platform, and it is the half that is red"
  - id: c3
    text: "A positive control on a fine-tick host still turns red when the manifest build is moved back outside the timeout scope"
    state: not-met
    owner: core
    note: "without this arm the fix is indistinguishable from disabling the test. Do NOT widen the *3 factor instead - raising the constant until it stops failing is the move this test's own comments forbid"
  - id: c4
    text: "The retries=0 override on this test survives the fix"
    state: met
    evidence: "file:.config/nextest.toml:599"
    owner: core
    note: "the override naming this test by literal operand is correct and should be kept - the failure must not become retryable. A separate script fails if any of the three named tests stops being a literal operand of a retries=0 predicate"
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
