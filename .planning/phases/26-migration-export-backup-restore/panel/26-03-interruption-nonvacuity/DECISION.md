# Decision A — interruption non-vacuity

CHOSEN: both-legs-sound
BASIS: majority

## The vote

| Member | Verdict |
|---|---|
| codex (gpt-5.6-sol) | `both-legs-sound` |
| gemini (3.1-pro-preview) | `both-legs-sound` |
| kimi (K3) | `both-legs-sound` |
| internal adversarial | `both-legs-sound`, with a recorded finding |

Unanimous, and all four independently separated the same two questions: whether
the kill landed mid-flight and whether recovery was exact (both measured, both
yes, on both platforms), versus whether the *uncatchability* of that kill was
independently instrumented (yes on Linux, no on Windows).

## The measurement that binds this choice

| Measurement | Linux | Windows |
|---|---|---|
| `MIDFLIGHT-JOURNAL-OPEN` | yes | yes |
| `MIDFLIGHT-TARGET-INTERMEDIATE` | yes | yes |
| `completed_before_kill` | no | no |
| `DIGEST-EQUAL` | yes | yes |
| `DIGEST-ALGO` byte-equal across platforms | yes | yes |
| Negative control detected its late kill | yes, exit 9 | yes, exit 9 |
| Handler control fired | **yes** | **no** |

`both-legs-sound` is bound by the plan's own gate to all four of mid-flight
landing and digest equality on both platforms. Every one of those eight cells is
measured and green. No cell was inferred.

## FINDING: F26-03-E (medium) — the Windows handler control does not fire, so `fired=no` there is corroborated by Win32 semantics rather than instrumented

On Windows the probe is proven to ARM (the binary writes an armed marker only
after a handler registers; the marker is present) and the catchable console
CTRL_C event is proven to be DELIVERED (the delivery helper exits 0 and the
target process dies, leaving `DIGEST-EQUAL: no`). The probe nevertheless records
nothing, because the process is torn down before the write lands.

Consequence, stated exactly: on Windows the rollback-exactness claim is fully
measured and holds, and the separate claim that `TerminateProcess` is uncatchable
rests on documented Win32 semantics plus a delivered-but-unrecorded event — not
on a fired probe. This does not make the leg vacuous, because none of the four
load-bearing measurements depends on the probe.

Severity **medium**: it degrades corroboration of a property that is guaranteed
by the platform's own documented semantics, and it does not touch the rollback
claim. Logged to BACKLOG, non-blocking, per the phase's severity policy.

Two real instrument defects were found and FIXED while establishing this, and
both were of the self-passing family:

1. Both proof scripts printed the literal string `installed=yes`, so the line
   asserted an armed probe whether or not one existed. It is now read from a
   marker the binary writes only on successful registration.
2. The Windows probe armed four signal handlers through a single irrefutable
   binding, so a single registration failure silently disarmed the entire probe
   and guaranteed `fired=no`. Each is now armed independently.

## Dissent, in its own terms

No member dissented from the option. The internal pass argued the strongest
available case for `windows-leg-vacuous` — that `fired=no` on Windows is the
same shape as this program's two recorded blind-instrument failures, a zero from
an instrument not shown able to produce anything else — and that argument is
recorded in `internal.md` rather than disposed of. It is answered, not dismissed,
by two measurements taken specifically to answer it: the probe demonstrably arms,
and the event is demonstrably delivered.

`windows-exactness-gap` was rejected on evidence, not preference: it requires the
Windows digests to differ, and they are equal (`719ee8c7…` pre and post).
`linux-leg-vacuous` was rejected because the Linux capture shows an open journal,
an intermediate target and `completed_before_kill=no`.
