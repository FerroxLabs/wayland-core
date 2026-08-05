# Internal adversarial pass — argued AGAINST `both-legs-sound`

I walked in believing `both-legs-sound`, because I had just made the Windows leg
run for the first time and was invested in it. This pass argues against that.

## The case for `windows-leg-vacuous`

The phase's own standard is not "the kill landed mid-flight and the digests
matched". It is that each leg's non-vacuity be **independently established**, and
the handler probe is one of the two instruments that establishes it. The
`KILL-HANDLER-PROBE: installed=yes fired=no` line in the Windows main capture is
presented as a measurement of uncatchability. On Windows it is not one. The
paired control that would give it meaning does not fire, and the whole reason
that control exists — stated in this repo's own source comments — is that
`fired=no` is otherwise equally consistent with a probe that never works.

So the Windows main capture contains a line that *looks* like a measurement and
is not. That is precisely the defect class this program has been bitten by
repeatedly: `AGREE scanner=0 manual=0` taken while the instrument was blind, and
a Windows orphan scan reporting a measured zero because `tasklist` cannot print
command lines. `fired=no` on Windows is today the same shape — a zero produced by
an instrument that has not been shown able to produce anything else.

The honest reading, on that argument, is `windows-leg-vacuous`: re-run it when
the control can fire, and do not bank the leg until then.

## Why I nevertheless do not adopt it

Two things break the analogy, and both are measurable rather than rhetorical.

First, the instrument is no longer unexamined. `installed=` was a hardcoded
string `yes` in **both** scripts until this lane changed it; it is now read from
a marker the binary writes only after a handler genuinely registers. On Windows
that marker is present, so the probe demonstrably arms. And the control run shows
the event is demonstrably delivered: the helper exits 0 and the target process
dies, leaving `DIGEST-EQUAL: no`. What is missing is only the probe's *record* of
an event that provably occurred — the process is torn down before the write
lands. That is a race in the instrument's reporting path, not a blind instrument.

Second, and decisively, `fired=no` is not what carries the rollback claim.
`MIDFLIGHT-JOURNAL-OPEN: yes`, `MIDFLIGHT-TARGET-INTERMEDIATE: yes` and
`completed_before_kill=no` establish that the kill landed mid-flight, and
`DIGEST-EQUAL: yes` establishes that recovery restored the exact prior tree.
None of those four depends on the handler probe. Uncatchability is additionally
guaranteed by documented Win32 semantics: `TerminateProcess` cannot be trapped,
masked or deferred. The probe was only ever corroboration.

## The case against `windows-exactness-gap`, for completeness

`windows-exactness-gap` is unavailable on the evidence, and choosing it would be
the rounding-up the plan explicitly forbids in the opposite direction. It
requires the Windows digests to DIFFER. They are equal
(`719ee8c7…` pre and post). Exactness was reached; there is no gap to record.

## Conclusion

I concur with `both-legs-sound`, but only with the Windows handler-control
failure recorded as an open finding in its own right rather than absorbed. The
uncatchability *claim* on Windows rests on Win32 semantics and on a delivered-
but-unrecorded event, not on a fired probe — and the summary must say exactly
that rather than let a unanimous panel imply the control passed.
