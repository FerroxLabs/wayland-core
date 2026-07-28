# 26-02 Windows quarantine leg — RUN. The paired live proof holds on Windows.

26-02 recorded its Windows leg as unrun, on the same false `seandesktop` host-access blocker
that stopped three other legs. The host is reachable as `SeanD` (see
`.planning/phases/28-native-cross-platform-certification/evidence/28-03-windows-requeue/HOST-ACCESS.md`).

## What was run, and why it is the same proof rather than a weaker one

The Linux proof is **paired and live**: the REAL binary is driven through a REAL agent turn
against a scripted mock provider, with the negative leg asserting that the Skill tool **ran and
reported the skill unavailable**, and a positive control using the **same payload and the same
turn**, differing only by `migrate promote`.

A Windows leg that merely checked where the quarantined bytes sit would be strictly weaker —
and would not have caught either of the two false greens 26-02's own construction caught. So
this runs **the same two tests**, `t19` and `t20`, on Windows.

```
test t19_live_negative_leg_quarantined_payload_does_not_execute        ... ok
test t20_live_positive_control_same_payload_executes_once_promoted     ... ok
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 141.82s
```

Host `SeanD@seandesktop`, account `seand`, lane HEAD `eecfc331`, via a scheduled task with an
explicit exit marker. **141.82 s of real work** — the greens are not vacuous.

## The positive control is what makes the negative leg mean anything

`t20` passing is the load-bearing half. It proves the payload **does** execute once an operator
promotes it, so `t19`'s absent sentinel is *containment* rather than a payload that never loads,
never parses or never gets discovered. Without it the negative leg carries no information.

### The portability precondition that had to be measured FIRST

The payload's directive is `` `touch <sentinel>` `` and **`touch` is not a cmd builtin**. Had it
not resolved on Windows, `t20` would have failed and the honest reading would have been *"the
fixture cannot run here"* — not *"containment leaked"*. Measured on this box **before** the run:

```
where touch            -> C:\Program Files\Git\usr\bin\touch.exe   (rc=0)
cmd /C "touch <path>"  -> rc=0, file created: True
```

and re-asserted at run time inside the runner (`Q_TOUCH_RC=0`, `Q_TOUCH_CREATED=True`), because
SYSTEM and the interactive user do not share a `PATH`.

## Count difference against Linux, stated rather than smoothed over

| Family | Tests run | Result |
|---|---|---|
| linux (26-02) | **29** (22 authored + 7 support self-tests) | 29 passed |
| windows (this leg) | **26** (22 authored + 4 support self-tests) | 26 passed |

**All 22 authored tests `t1`–`t22` ran and passed on both.** The delta is 3 support-module
self-tests, in the PTY helper, which is Unix-only by `#[cfg]` and therefore not compiled on
Windows. No authored assertion is missing on Windows.

## A harness bug this leg caught in itself, via elapsed time

The first attempt passed both test names to a single `cargo test` invocation. Cargo rejects a
second positional filter:

```
cargo.exe : error: unexpected argument 't20_live_positive_control_same_payload_executes_once_promoted' found
Q_PAIRED_SECONDS=0.08
```

**0.08 s.** Far too fast for a cargo build plus two live agent turns — the same "too fast to
have run" signal that stopped the `KR-01` leg mis-filing a finding. The suite invocation in the
same script then ran everything anyway, which is where the numbers above come from, and the
corrected runner splits the two legs into separate invocations so each carries its own recorded
exit status instead of sharing one.

## Verdict

**26-02's Windows leg: MET.** Import quarantine is inert by placement on Windows, proven by the
same paired live construction as Linux, with the positive control firing.
